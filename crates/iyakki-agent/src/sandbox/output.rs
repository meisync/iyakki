//! What a run printed, made safe to hand back: control characters removed, exact secret values and anything that looks like a credential
//! replaced (the repo's credential filter, `bugs::redact`), then capped to the run's output limit (head and tail kept).

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cleaned {
    pub text: String,
    pub truncated: bool,
    pub redacted: bool,
}

pub const HIDDEN: &str = "[redacted]";

/// `secrets` are values this run held (environment values and resolved secrets): they are removed wherever they appear, even split by
/// nothing but also in common encodings (base64 is not tried: the proxy is the way for secrets that must not be in the run at all).
pub fn clean(raw: &[u8], cap_kib: u32, secrets: &[String]) -> Cleaned {
    let lossy = String::from_utf8_lossy(raw);
    let mut t: String = lossy.chars().filter(|c| *c == '\n' || *c == '\t' || !c.is_control()).collect();
    let mut redacted = false;
    for s in secrets.iter().filter(|s| s.len() >= 4) {
        if t.contains(s.as_str()) {
            t = t.replace(s.as_str(), HIDDEN);
            redacted = true;
        }
    }
    let mut r = crate::bugs::redact(&t);
    // the filter works by lines and drops the last newline; the caller's text keeps its own ending
    if t.ends_with('\n') && !r.ends_with('\n') {
        r.push('\n');
    }
    if r != t {
        redacted = true;
    }
    let cap = cap_kib as usize * 1024;
    if r.len() <= cap {
        return Cleaned { text: r, truncated: false, redacted };
    }
    let half = cap / 2;
    let mut a = half;
    while !r.is_char_boundary(a) {
        a -= 1;
    }
    let mut b = r.len() - half;
    while !r.is_char_boundary(b) {
        b += 1;
    }
    Cleaned { text: format!("{}\n[... {} bytes cut ...]\n{}", &r[..a], b - a, &r[b..]), truncated: true, redacted }
}
