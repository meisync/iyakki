//! Artifacts from a finished Job (F-SBX-03). The container is gone when the Job has ended, so plain `kubectl` cannot copy files out of it.
//! The run wrapper ([`wrap`]) prints the files the request named as one framed line each at the end of the log, and [`split`] takes those
//! lines out of the log again:
//!
//! ```text
//! @@iyakki-artifact@@ <base64 of the file name> <base64 of the contents>
//! ```
//!
//! Only files the request named are wrapped (the wrapper needs `sh`, `base64` and `tr` in the image, which a minimal image may not have;
//! a request without names runs the command unchanged). Only the names asked for are accepted back, whatever the program printed, and a
//! frame that does not decode is dropped. The text that is left is what the program printed.

use crate::models::b64_decode;
use crate::story::base64_encode;

pub const MARK: &str = "@@iyakki-artifact@@";
/// Extra bytes of log read beyond the output cap so the frames at the end are reached (4 MiB of files is about 5.4 MiB of text).
pub const BUDGET: usize = 6 * 1024 * 1024;
/// At most this many frames are taken from one log.
pub const MAX_FRAMES: usize = 40;
const OUT_DIR: &str = "/workspace/out";

/// A name the wrapper may be given: a relative path of plain characters under the out folder.
pub fn name_ok(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 200
        && !n.starts_with(['/', '.', '-'])
        && !n.contains("..")
        && !n.contains("//")
        && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
}

/// The command with the wrapper around it. `None` when there is nothing to wrap (no names, or a name that is not allowed).
pub fn wrap(argv: &[String], names: &[String]) -> Option<Vec<String>> {
    if argv.is_empty() || names.is_empty() || names.len() > MAX_FRAMES || !names.iter().all(|n| name_ok(n)) {
        return None;
    }
    let list = names.iter().map(|n| format!("'{n}'")).collect::<Vec<_>>().join(" ");
    let script = format!(
        "\"$@\"; c=$?; for f in {list}; do if [ -f \"{OUT_DIR}/$f\" ]; then printf '%s %s ' {MARK} \"$(printf %s \"$f\" | base64 | tr -d '\\n')\"; base64 < \"{OUT_DIR}/$f\" | tr -d '\\n'; printf '\\n'; fi; done; exit $c"
    );
    let mut v = vec!["sh".to_string(), "-c".into(), script, "iyakki-run".into()];
    v.extend(argv.iter().cloned());
    Some(v)
}

/// One frame line, as the wrapper prints it (used by tests and by anything that stands in for the wrapper).
pub fn frame(name: &str, data: &[u8]) -> String {
    format!("{MARK} {} {}\n", base64_encode(name.as_bytes()), base64_encode(data))
}

/// The log without its frames, and the files the frames held (`out/<name>`), only for the `wanted` names, in the order printed.
pub fn split(log: &[u8], wanted: &[String]) -> (Vec<u8>, Vec<(String, Vec<u8>)>) {
    let mut text = Vec::with_capacity(log.len());
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mark = MARK.as_bytes();
    let mut taken = 0;
    for line in log.split_inclusive(|b| *b == b'\n') {
        if !line.starts_with(mark) || line.get(mark.len()) != Some(&b' ') {
            text.extend_from_slice(line);
            continue;
        }
        taken += 1;
        if taken > MAX_FRAMES {
            continue;
        }
        let body = String::from_utf8_lossy(&line[mark.len() + 1..]).trim().to_string();
        let mut parts = body.splitn(2, ' ');
        let (Some(n), Some(d)) = (parts.next(), parts.next()) else { continue };
        let Some(name) = b64_decode(n).and_then(|b| String::from_utf8(b).ok()) else { continue };
        let Some(data) = b64_decode(d) else { continue };
        if wanted.iter().any(|w| *w == name) && !files.iter().any(|(p, _)| *p == format!("out/{name}")) {
            files.push((format!("out/{name}"), data));
        }
    }
    (text, files)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn the_wrapper_runs_the_command_unchanged_inside_and_keeps_its_exit_code() {
        let w = wrap(&v(&["python3", "run.py", "a b"]), &v(&["result.txt", "sub/chart.png"])).unwrap();
        assert_eq!(&w[..2], ["sh", "-c"]);
        assert_eq!(&w[3..], ["iyakki-run", "python3", "run.py", "a b"], "the program and its arguments are separate words, never part of the script");
        assert!(w[2].starts_with("\"$@\"; c=$?; for f in 'result.txt' 'sub/chart.png'; do"), "{}", w[2]);
        assert!(w[2].ends_with("done; exit $c") && w[2].contains(MARK) && w[2].contains("/workspace/out/$f"));
        assert!(!w[2].contains("python3") && !w[2].contains("a b"));
    }

    #[test]
    fn nothing_is_wrapped_without_names_or_with_a_name_that_could_escape_the_out_folder() {
        assert!(wrap(&v(&["x"]), &[]).is_none());
        assert!(wrap(&[], &v(&["a"])).is_none());
        for bad in ["", "/etc/passwd", "../x", "a/../b", "a'b", "a b", "$(x)", "-n", ".hidden", "a;b", "a//b"] {
            assert!(wrap(&v(&["x"]), &v(&[bad])).is_none(), "{bad}");
        }
        assert!(wrap(&v(&["x"]), &(0..41).map(|i| format!("f{i}")).collect::<Vec<_>>()).is_none());
        assert!(name_ok("report-1.2_final/out.csv"));
    }

    #[test]
    fn frames_are_taken_out_of_the_log_and_the_text_is_what_the_program_printed() {
        let mut log = b"line one\nline two\n".to_vec();
        log.extend(frame("result.txt", b"data\nwith newline").as_bytes());
        log.extend(frame("sub/chart.png", &[0, 255, 1, 2]).as_bytes());
        log.extend(frame("other.txt", b"not asked for").as_bytes());
        let (text, files) = split(&log, &v(&["result.txt", "sub/chart.png"]));
        assert_eq!(text, b"line one\nline two\n");
        assert_eq!(files, vec![("out/result.txt".to_string(), b"data\nwith newline".to_vec()), ("out/sub/chart.png".to_string(), vec![0, 255, 1, 2])]);
    }

    #[test]
    fn damaged_duplicate_and_surplus_frames_are_dropped_and_a_log_without_frames_is_untouched() {
        let plain = b"no frames here\n@@iyakki-artifactX not a frame\nlast line without newline".to_vec();
        assert_eq!(split(&plain, &v(&["a"])), (plain.clone(), vec![]));
        let mut log = Vec::new();
        log.extend(format!("{MARK} !!!! ????\n{MARK} onlyonepart\n").as_bytes());
        log.extend(frame("a", b"first").as_bytes());
        log.extend(frame("a", b"second").as_bytes());
        let (text, files) = split(&log, &v(&["a"]));
        assert!(text.is_empty());
        assert_eq!(files, vec![("out/a".to_string(), b"first".to_vec())]);
        let mut many = Vec::new();
        for i in 0..MAX_FRAMES + 5 {
            many.extend(frame(&format!("f{i}"), b"x").as_bytes());
        }
        let wanted: Vec<String> = (0..MAX_FRAMES + 5).map(|i| format!("f{i}")).collect();
        assert_eq!(split(&many, &wanted).1.len(), MAX_FRAMES);
    }
}
