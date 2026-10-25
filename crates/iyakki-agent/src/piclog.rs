//! The picture log of TALES: one JSON line per event in `Stories/<id>/pictures.jsonl`, so a poor or missing picture can be
//! explained. It sits beside the older `pictures.log` (which is kept as it was). The log stays on this computer: it holds the
//! person's own story text (the prompts), never an API key.
//!
//! * [`PicEvent`]: one thing that happened (a picture request and its answer, a picture skipped, a cap, a fall back to SVG);
//! * [`append`], [`read_all`], [`clear`]: the files, size-bounded (1 MB each, the newest three kept);
//! * [`event_text`], [`log_text`]: the plain text the Picture log view shows and Copy log copies;
//! * [`redact_headers`], [`scrub`]: what keeps secrets out of the log.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The file the log is written to; older files are `pictures.jsonl.1` (newer) and `pictures.jsonl.2` (oldest).
pub const LOG_FILE: &str = "pictures.jsonl";
/// One file grows to this many bytes, then the files are rotated.
pub const MAX_FILE_BYTES: u64 = 1 << 20;
/// How many files are kept.
pub const KEEP_FILES: usize = 3;

/// What kind of event a line is.
pub mod kind {
    /// A request to a picture model and what came back.
    pub const REQUEST: &str = "request";
    /// A picture that was not asked for (memory, cancelled).
    pub const SKIPPED: &str = "skipped";
    /// A finished picture that was not kept (a cap).
    pub const NOT_KEPT: &str = "not kept";
    /// The rest of the story is drawn as SVG instead.
    pub const SVG_FALLBACK: &str = "svg fallback";
    /// The picture model is not on the server: nothing was asked of it.
    pub const NOT_DOWNLOADED: &str = "not downloaded";
    /// A download of the picture model: started, progress, finished or failed.
    pub const PULL: &str = "pull";
    /// A picture call that is still running: how long, and what the server says.
    pub const WAITING: &str = "waiting";
    /// A picture call that was given up after the time limit.
    pub const TIMEOUT: &str = "timeout";
}

/// The extra detail of the Verbose switch: the request headers (no secrets) and the server's health at the time.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verbose {
    pub request: String,
    pub request_headers: Vec<String>,
    pub response_headers: String,
    pub health: String,
}

/// One event of the picture log (one line of `pictures.jsonl`). Fields that do not apply are empty.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PicEvent {
    /// Seconds since 1970.
    pub ts: u64,
    pub kind: String,
    pub story: String,
    /// Which picture of the story (from 1), 0 when it is about no one picture.
    pub picture: usize,
    pub scene: String,
    /// How many times this picture was asked for so far (1 is the first try); the retries are `attempt - 1`.
    pub attempt: u32,
    pub provider: String,
    pub model: String,
    /// `WxH` as sent.
    pub size: String,
    pub steps: Option<u32>,
    pub seed: Option<u64>,
    pub prompt: String,
    pub negative: String,
    /// How the negative prompt went: `field`, `in the prompt text` or `not sent`.
    pub negative_sent: String,
    pub ms: u64,
    pub status: Option<u16>,
    pub error: String,
    pub bytes: u64,
    /// `png`, `jpg`, `webp`, `gif` when the picture was valid; `invalid` when it was not; empty when nothing came back.
    pub format: String,
    /// Why the picture became an SVG, when it did.
    pub svg_fallback: String,
    /// A decision that was taken (`skipped: free memory low ...`, `not kept: ...`).
    pub decision: String,
    pub verbose: Option<Verbose>,
    /// The style drawn (`photo`, `storybook`, ...), which picture of that style (from 1), and whether Realistic pictures was on.
    pub style: String,
    pub variant: u32,
    pub realistic: bool,
    /// The guidance (`cfg_scale`) asked for, when any.
    pub guidance: Option<f32>,
}

// ------------------------------------------------------------------------------------------------------------ secrets

/// The headers with every secret value replaced: `Authorization`, and any header whose name says key, token, secret or auth.
pub fn redact_headers(h: &[(String, String)]) -> Vec<(String, String)> {
    h.iter().map(|(k, v)| (k.clone(), if secret_name(k) { "***".to_string() } else { v.clone() })).collect()
}

fn secret_name(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    ["authorization", "key", "token", "secret", "auth", "cookie", "password"].iter().any(|w| k.contains(w))
}

/// The secret values among the headers (what [`redact_headers`] hides), for [`scrub`]. The words `Bearer` etc. are not secret.
pub fn secret_values(h: &[(String, String)]) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for (k, val) in h.iter().filter(|(k, _)| secret_name(k)) {
        let _ = k;
        v.push(val.clone());
        if let Some((_, rest)) = val.split_once(' ') {
            v.push(rest.to_string());
        }
    }
    v.retain(|s| s.len() >= 6);
    v
}

/// The text with every occurrence of a secret value replaced by `***`.
pub fn scrub(text: &str, secrets: &[String]) -> String {
    let mut t = text.to_string();
    for s in secrets.iter().filter(|s| !s.is_empty()) {
        t = t.replace(s.as_str(), "***");
    }
    t
}

/// An address without a user name and password in it.
pub fn strip_userinfo(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let (auth, path) = rest.split_once('/').map_or((rest, ""), |(a, p)| (a, p));
            let host = auth.rsplit_once('@').map_or(auth, |(_, h)| h);
            if path.is_empty() { format!("{scheme}://{host}") } else { format!("{scheme}://{host}/{path}") }
        }
        None => url.to_string(),
    }
}

// ------------------------------------------------------------------------------------------------------------ files

fn file(dir: &Path, n: usize) -> PathBuf {
    if n == 0 { dir.join(LOG_FILE) } else { dir.join(format!("{LOG_FILE}.{n}")) }
}

/// Add events to the log in `dir`, one line each, rotating when the file would pass `max` bytes (the newest [`KEEP_FILES`] files
/// are kept: `pictures.jsonl`, `.1`, `.2`).
pub fn append_bounded(dir: &Path, events: &[PicEvent], max: u64) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    for e in events {
        let mut line = serde_json::to_string(e).unwrap_or_default();
        line.push('\n');
        let cur = file(dir, 0);
        let size = std::fs::metadata(&cur).map_or(0, |m| m.len());
        if size > 0 && size + line.len() as u64 > max {
            let _ = std::fs::remove_file(file(dir, KEEP_FILES - 1));
            for n in (0..KEEP_FILES - 1).rev() {
                if file(dir, n).exists() {
                    std::fs::rename(file(dir, n), file(dir, n + 1))?;
                }
            }
        }
        std::fs::OpenOptions::new().create(true).append(true).open(&cur)?.write_all(line.as_bytes())?;
    }
    Ok(())
}

/// [`append_bounded`] with the usual limit of 1 MB a file.
pub fn append(dir: &Path, events: &[PicEvent]) -> std::io::Result<()> {
    append_bounded(dir, events, MAX_FILE_BYTES)
}

/// Every event kept, oldest first. A line that is not an event (a damaged one) is skipped.
pub fn read_all(dir: &Path) -> Vec<PicEvent> {
    let mut out = Vec::new();
    for n in (0..KEEP_FILES).rev() {
        if let Ok(t) = std::fs::read_to_string(file(dir, n)) {
            out.extend(t.lines().filter_map(|l| serde_json::from_str::<PicEvent>(l).ok()));
        }
    }
    out
}

/// Delete the log files in `dir`.
pub fn clear(dir: &Path) {
    for n in 0..KEEP_FILES {
        let _ = std::fs::remove_file(file(dir, n));
    }
}

/// The names of the log files (to delete with a story).
pub fn file_names() -> Vec<String> {
    (0..KEEP_FILES).map(|n| if n == 0 { LOG_FILE.to_string() } else { format!("{LOG_FILE}.{n}") }).collect()
}

// ------------------------------------------------------------------------------------------------------------ text

/// "2026-10-24 13:05:09" (UTC) from seconds since 1970.
pub fn time_text(secs: u64) -> String {
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let s = secs % 86_400;
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
}

/// One event as plain text lines (the same lines the view shows and Copy log copies).
pub fn event_text(e: &PicEvent) -> Vec<String> {
    let who = if e.picture == 0 { e.story.clone() } else { format!("{} · picture {} ({})", e.story, e.picture, e.scene) };
    let mut v = vec![format!("{}  [{}]  {who}", time_text(e.ts), e.kind)];
    if !e.decision.is_empty() {
        v.push(format!("  decision: {}", e.decision));
    }
    if !e.model.is_empty() {
        v.push(format!("  model: {} on {} · size {} · steps {} · guidance {} · seed {} · attempt {} (retries {})", e.model, e.provider, or_dash(&e.size), e.steps.map_or("-".into(), |s| s.to_string()), e.guidance.map_or("model's own".into(), |g| g.to_string()), e.seed.map_or("-".into(), |s| s.to_string()), e.attempt.max(1), e.attempt.saturating_sub(1)));
    }
    if !e.style.is_empty() {
        v.push(format!("  style: {} · picture {} of this style · realistic pictures: {}", e.style, e.variant.max(1), if e.realistic { "on" } else { "off" }));
    }
    if !e.prompt.is_empty() {
        v.push(format!("  prompt: {}", e.prompt));
        v.push(format!("  negative prompt: {} ({})", or_dash(&e.negative), or_dash(&e.negative_sent)));
    }
    if e.kind == kind::REQUEST {
        v.push(format!("  call: {} ms · HTTP {} · {} bytes · picture: {}", e.ms, e.status.map_or("none".into(), |s| s.to_string()), e.bytes, if e.format.is_empty() { "none".to_string() } else if e.format == "invalid" { "not a valid PNG, JPEG or WebP".into() } else { format!("valid {}", e.format) }));
    }
    if !e.error.is_empty() {
        v.push(format!("  server error: {}", e.error));
    }
    if !e.svg_fallback.is_empty() {
        v.push(format!("  SVG fallback: {}", e.svg_fallback));
    }
    if let Some(x) = &e.verbose {
        v.push(format!("  request: {}", x.request));
        for h in &x.request_headers {
            v.push(format!("  request header: {h}"));
        }
        v.push(format!("  response headers: {}", x.response_headers));
        v.push(format!("  server health: {}", x.health));
    }
    v
}

fn or_dash(s: &str) -> &str {
    if s.is_empty() { "-" } else { s }
}

/// The whole log as text, oldest first, a blank line between events.
pub fn log_text(events: &[PicEvent]) -> String {
    events.iter().map(|e| event_text(e).join("\n")).collect::<Vec<_>>().join("\n\n")
}

// ------------------------------------------------------------------------------------------------------------ from a request

/// What the thread that asked for a picture knows besides the trace: the story, the picture, the prompt as built.
pub struct RequestInfo<'a> {
    pub ts: u64,
    pub story: &'a str,
    pub picture: usize,
    pub scene: &'a str,
    pub attempt: u32,
    pub prompt: &'a str,
    pub negative: &'a str,
    pub negative_sent: &'a str,
    pub verbose: bool,
    pub style: &'a str,
    pub variant: u32,
    pub realistic: bool,
    pub guidance: Option<f32>,
}

/// The event of one picture request, from what [`crate::providers::ImageTrace`] says was sent and came back. `rejected` is why
/// the picture was refused after it arrived (a decode, size or pixel check), `svg_fallback` why the story went on with SVG.
pub fn request_event(t: &crate::providers::ImageTrace, i: &RequestInfo, rejected: Option<&str>, svg_fallback: &str) -> PicEvent {
    let verbose = i.verbose.then(|| Verbose {
        request: format!("POST {} (model, prompt, n 1, size {}x{}, response_format b64_json{}{})", if t.url.is_empty() { "-" } else { &t.url }, t.width, t.height, t.steps.map_or(String::new(), |s| format!(", steps {s}")), t.seed.map_or(String::new(), |s| format!(", seed {s}")) + &t.guidance.map_or(String::new(), |g| format!(", cfg_scale {g}"))),
        request_headers: t.request_headers.iter().map(|(k, v)| format!("{k}: {v}")).collect(),
        response_headers: "not available: only the status and the body of the answer are read".into(),
        health: t.health.clone().unwrap_or_else(|| "not read".into()),
    });
    PicEvent {
        ts: i.ts,
        kind: kind::REQUEST.into(),
        story: i.story.into(),
        picture: i.picture,
        scene: i.scene.into(),
        attempt: i.attempt,
        provider: t.provider.clone(),
        model: t.model.clone(),
        size: if t.width > 0 { format!("{}x{}", t.width, t.height) } else { String::new() },
        steps: t.steps,
        seed: t.seed,
        prompt: i.prompt.into(),
        negative: i.negative.into(),
        negative_sent: i.negative_sent.into(),
        ms: t.ms,
        status: t.status,
        error: t.error.clone(),
        bytes: t.image_bytes.max(if t.format.is_empty() { t.response_bytes } else { 0 }),
        format: if rejected.is_some() { "invalid".into() } else { t.format.clone() },
        svg_fallback: svg_fallback.into(),
        decision: rejected.map(|r| format!("picture refused: {r}")).unwrap_or_default(),
        verbose,
        style: i.style.into(),
        variant: i.variant,
        realistic: i.realistic,
        guidance: i.guidance,
    }
}

/// An event that is only a decision (a picture skipped, not kept, cancelled, or the SVG fallback).
pub fn decision_event(ts: u64, kind: &str, story: &str, picture: usize, scene: &str, decision: &str) -> PicEvent {
    PicEvent { ts, kind: kind.into(), story: story.into(), picture, scene: scene.into(), decision: decision.into(), ..Default::default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-piclog-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn ev(n: u64, kind: &str) -> PicEvent {
        PicEvent { ts: 1_792_000_000 + n, kind: kind.into(), story: "The Fox".into(), picture: n as usize, scene: format!("Part {n}"), ..Default::default() }
    }

    #[test]
    fn story_piclog_events_are_kept_in_order_with_their_fields() {
        let d = dir("order");
        let mut a = ev(1, kind::REQUEST);
        a.provider = "gpu".into();
        a.model = "SDXL-Base-1.0".into();
        a.size = "1024x1024".into();
        a.steps = Some(20);
        a.seed = Some(7);
        a.prompt = "a fox".into();
        a.negative = "blurry".into();
        a.negative_sent = "in the prompt text".into();
        a.ms = 1234;
        a.status = Some(200);
        a.bytes = 4096;
        a.format = "png".into();
        let mut b = ev(2, kind::SKIPPED);
        b.decision = "skipped: free memory low (900 MB free, about 3072 MB needed)".into();
        let mut c = ev(3, kind::REQUEST);
        c.status = Some(500);
        c.error = "out of memory".into();
        c.svg_fallback = "the server said: out of memory".into();
        append(&d, &[a.clone(), b.clone()]).unwrap();
        append(&d, std::slice::from_ref(&c)).unwrap();
        let back = read_all(&d);
        assert_eq!(back, vec![a.clone(), b, c.clone()], "oldest first, every field back");
        let t = log_text(&back);
        for want in ["SDXL-Base-1.0 on gpu", "1024x1024", "steps 20", "seed 7", "prompt: a fox", "negative prompt: blurry (in the prompt text)", "1234 ms", "HTTP 200", "4096 bytes", "valid png", "skipped: free memory low", "server error: out of memory", "SVG fallback: the server said", "The Fox · picture 3 (Part 3)"] {
            assert!(t.contains(want), "{want} in {t}");
        }
        assert!(t.find("picture 1").unwrap() < t.find("picture 2").unwrap() && t.find("picture 2").unwrap() < t.find("picture 3").unwrap());
        let mut bad = c;
        bad.format = "invalid".into();
        assert!(event_text(&bad).join("\n").contains("not a valid PNG, JPEG or WebP"));
        assert!(!event_text(&a).join("\n").contains("request header"), "no verbose detail unless it was asked for");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn story_piclog_secrets_never_reach_the_log() {
        let h = vec![("Content-Type".to_string(), "application/json".to_string()), ("Authorization".to_string(), "Bearer sk-SECRET-123456".to_string()), ("x-api-key".to_string(), "KEY-987654".to_string())];
        let r = redact_headers(&h);
        assert_eq!(r[0].1, "application/json");
        assert!(r[1].1 == "***" && r[2].1 == "***", "{r:?}");
        let secrets = secret_values(&h);
        let said = scrub("HTTP 401: bad key sk-SECRET-123456 and KEY-987654", &secrets);
        assert!(!said.contains("SECRET") && !said.contains("987654") && said.contains("***"), "{said}");
        assert_eq!(strip_userinfo("http://admin:pw@10.0.0.5:13305/api/v1"), "http://10.0.0.5:13305/api/v1");
        assert_eq!(strip_userinfo("http://host:1/x"), "http://host:1/x");
    }

    #[test]
    fn story_piclog_rotates_at_the_limit_keeps_three_files_and_clears() {
        let d = dir("rotate");
        let mut e = ev(1, kind::REQUEST);
        e.prompt = "x".repeat(100);
        // a small limit so the test stays tiny: about three events a file (each is about 450 bytes)
        for n in 0..20u64 {
            e.ts = n;
            append_bounded(&d, std::slice::from_ref(&e), 1500).unwrap();
        }
        for n in 0..KEEP_FILES {
            let p = file(&d, n);
            assert!(p.exists() && std::fs::metadata(&p).unwrap().len() <= 1500, "{n}");
        }
        assert!(!d.join("pictures.jsonl.3").exists(), "only three files");
        let back = read_all(&d);
        assert!(back.len() >= 6 && back.len() < 20, "the oldest were dropped: {}", back.len());
        assert_eq!(back.last().unwrap().ts, 19, "the newest is kept");
        assert!(back.windows(2).all(|w| w[0].ts < w[1].ts), "still in order across the files");
        assert_eq!(MAX_FILE_BYTES, 1 << 20);
        clear(&d);
        assert!(read_all(&d).is_empty() && !file(&d, 0).exists());
        assert_eq!(file_names(), vec!["pictures.jsonl", "pictures.jsonl.1", "pictures.jsonl.2"]);
        let _ = std::fs::remove_dir_all(&d);
    }


    #[test]
    fn story_piclog_verbose_adds_headers_without_secrets_and_the_health() {
        let t = crate::providers::ImageTrace {
            provider: "gpu".into(),
            model: "SDXL".into(),
            url: "http://gpu:13305/api/v1/images/generations".into(),
            width: 1024,
            height: 1024,
            steps: Some(20),
            seed: Some(3),
            request_headers: redact_headers(&[("Authorization".into(), "Bearer sk-SECRET-123456".into()), ("Content-Type".into(), "application/json".into())]),
            status: Some(200),
            ms: 900,
            image_bytes: 5000,
            format: "png".into(),
            health: Some("loaded models: SDXL · backend_url http://127.0.0.1:8001".into()),
            ..Default::default()
        };
        let info = |verbose| RequestInfo { ts: 5, story: "S", picture: 1, scene: "A", attempt: 2, prompt: "p", negative: "n", negative_sent: "in the prompt text", verbose, style: "photo", variant: 2, realistic: true, guidance: Some(6.0) };
        let quiet = request_event(&t, &info(false), None, "");
        assert!(quiet.verbose.is_none(), "verbose is off unless asked for");
        assert_eq!((quiet.size.as_str(), quiet.steps, quiet.seed, quiet.status, quiet.bytes, quiet.format.as_str(), quiet.attempt), ("1024x1024", Some(20), Some(3), Some(200), 5000, "png", 2));
        let loud = request_event(&t, &info(true), None, "");
        let text = event_text(&loud).join("\n");
        assert!(text.contains("Authorization: ***") && text.contains("Content-Type: application/json") && !text.contains("SECRET"), "{text}");
        assert!(text.contains("backend_url http://127.0.0.1:8001") && text.contains("response headers: not available") && text.contains("retries 1"), "{text}");
        assert!(!event_text(&quiet).join("\n").contains("Authorization"));
        let q = event_text(&quiet).join("\n");
        assert!(q.contains("style: photo · picture 2 of this style · realistic pictures: on") && q.contains("guidance 6"), "{q}");
        let bad = request_event(&t, &info(false), Some("not a picture"), "the model fell over");
        assert_eq!((bad.format.as_str(), bad.decision.as_str(), bad.svg_fallback.as_str()), ("invalid", "picture refused: not a picture", "the model fell over"));
        let d = decision_event(9, kind::SKIPPED, "S", 2, "B", "skipped: free memory low");
        assert_eq!((d.kind.as_str(), d.picture, d.decision.as_str()), ("skipped", 2, "skipped: free memory low"));
    }

    #[test]
    fn story_piclog_time_text_and_damaged_lines() {
        assert_eq!(time_text(0), "1970-01-01 00:00:00");
        assert_eq!(time_text(1_792_000_000), "2026-10-14 17:46:40");
        let d = dir("damaged");
        append(&d, &[ev(1, kind::SKIPPED)]).unwrap();
        std::fs::write(file(&d, 0), format!("not json\n{}\n", serde_json::to_string(&ev(2, kind::SKIPPED)).unwrap())).unwrap();
        assert_eq!(read_all(&d).len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }
}
