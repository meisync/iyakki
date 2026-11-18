//! Browser developer tools: "report a broken site" (key `!`). It writes one line to a file on this computer, for the person (or an
//! administrator, or the assistant) to look at later; nothing is sent anywhere. A line holds the time, the host (never the path or
//! the query), the status, whether scripts were allowed for the site, which engine drew the page and up to 20 of the page's notes
//! (script errors and blocked things) with query strings and fragments cut off. The file keeps the last 100 lines.

use std::path::{Path, PathBuf};

const KEEP: usize = 100;
const MAX_NOTES: usize = 20;
const MAX_NOTE_CHARS: usize = 200;

/// The file of one person's reports.
pub fn file_for(dir: &Path, user: &str) -> PathBuf {
    let safe: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    dir.join(format!("browser-reports-{safe}.jsonl"))
}

/// Cut `?query` and `#fragment` off every address-looking word of a note, so a token in a URL is not kept.
pub fn scrub(note: &str) -> String {
    let words: Vec<String> = note
        .split(' ')
        .map(|w| if w.contains("://") { w.split(['?', '#']).next().unwrap_or("").to_string() } else { w.to_string() })
        .collect();
    let s = words.join(" ");
    s.chars().take(MAX_NOTE_CHARS).collect()
}

/// One report line (JSON, no newline inside).
pub fn line(at: i64, host: &str, status: u16, scripts: bool, engine: &str, notes: &[String]) -> String {
    let notes: Vec<String> = notes.iter().take(MAX_NOTES).map(|n| scrub(n)).collect();
    serde_json::json!({"at": at, "host": host, "status": status, "scripts": scripts, "engine": engine, "notes": notes}).to_string()
}

/// Append a line; only the last [`KEEP`] lines stay. Returns the number of lines kept.
pub fn append(path: &Path, line: &str) -> Result<usize, String> {
    let old = std::fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<&str> = old.lines().filter(|l| !l.trim().is_empty()).collect();
    lines.push(line);
    let from = lines.len().saturating_sub(KEEP);
    let mut out = lines[from..].join("\n");
    out.push('\n');
    std::fs::write(path, out).map_err(|e| format!("could not save the report: {e}"))?;
    Ok(lines.len() - from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_keeps_the_host_and_the_notes_but_not_paths_queries_or_tokens() {
        let notes = vec!["script error in https://cdn.example.org/app.js?token=SECRET#x: x is not defined".to_string(), "blocked https://ads.example.net/t.js?id=7".to_string()];
        let l = line(1700000000, "shop.example.org", 200, true, "reader", &notes);
        assert!(l.contains("shop.example.org") && l.contains("x is not defined") && l.contains("https://cdn.example.org/app.js x is not defined"), "{l}");
        assert!(!l.contains("SECRET") && !l.contains("token") && !l.contains("id=7"), "{l}");
        let v: serde_json::Value = serde_json::from_str(&l).unwrap();
        assert_eq!(v["host"], "shop.example.org");
        assert_eq!(v["notes"].as_array().unwrap().len(), 2);
        // many notes are cut
        let many: Vec<String> = (0..50).map(|i| format!("note {i}")).collect();
        let l = line(1, "a.org", 404, false, "reader", &many);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&l).unwrap()["notes"].as_array().unwrap().len(), MAX_NOTES);
    }

    #[test]
    fn the_file_keeps_the_last_hundred_lines_and_a_name_cannot_leave_the_folder() {
        let d = std::env::temp_dir().join(format!("iyakki-brreport-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = file_for(&d, "../evil/amy");
        assert_eq!(p.parent().unwrap(), d.as_path());
        for i in 0..130 {
            append(&p, &line(i, "a.org", 200, true, "reader", &[])).unwrap();
        }
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(text.lines().count(), KEEP);
        assert!(text.lines().next().unwrap().contains("\"at\":30"), "the oldest 30 were dropped");
    }
}
