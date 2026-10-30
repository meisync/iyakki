//! The support bundle: what a tester sends when something is wrong, built so the person SEES it first.
//!
//! One plain text file with sections (version, services, hardware, the doctor's findings, the kept web-engine log, the boot log's
//! tail). [`build`] redacts every line; [`preview`] returns the exact text the file will hold, and [`save`] writes those same bytes
//! (one code path: what is shown is what is written). Nothing is uploaded: the person saves the file in their home folder or on a
//! USB stick and sends it themselves.
//!
//! **Redaction** ([`redact`]) removes, line by line: `Cookie`, `Set-Cookie` and `Authorization` header values (replaced by `***`); the value of anything
//! named like a secret (`password`, `token`, `api_key`, `secret`, `cookie`, a vault entry's name with its value, ...) in `name=value`,
//! `name: value` and JSON forms; the query string and fragment of every address and the `user:password@` part; credentials and keys
//! the chat's detector knows ([`crate::credguard`]: `sk-...`, `ghp_...`, JWTs, private keys, ...); `Bearer` and `Basic` tokens; long
//! hexadecimal strings; e-mail addresses; MAC addresses; IPv4 and IPv6 addresses (except loopback); the account name in `/home/<name>`
//! paths. A last pass runs the detector over the whole result again. Redaction errs on the side of removing too much.
//!
//! Page addresses and page content never get here: the engine log is cleaned when it is written ([`crate::enginelog`]).

use std::path::{Path, PathBuf};

/// The whole file never passes this size (the oldest log lines go first).
pub const MAX_BYTES: usize = 512 * 1024;
/// What replaces a removed value.
/// Short on purpose: the credential detector reads a longer word after `password=` as a value again.
pub const GONE: &str = "***";

const SECRET_NAMES: &[&str] = &["password", "passwd", "passphrase", "pwd", "secret", "token", "apikey", "api_key", "api-key", "cookie", "credential", "private_key", "privatekey", "sessionid", "session_id", "authorization", "bearer"];

// ------------------------------------------------------------------------------------------------ redaction

fn is_value_end(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | ',' | '&' | ';' | '}' | ')' | '<' | '>')
}

/// Header lines whose whole value is a secret.
fn header_values(line: &str) -> Option<String> {
    let t = line.trim_start();
    let lower = t.to_ascii_lowercase();
    for h in ["cookie:", "set-cookie:", "authorization:", "proxy-authorization:", "x-api-key:", "x-auth-token:"] {
        if lower.starts_with(h) {
            let lead = line.len() - t.len();
            return Some(format!("{}{} {GONE}", &line[..lead], &t[..h.len()]));
        }
    }
    None
}

/// `name=value`, `name: value`, `"name": "value"` where the name holds a secret word.
fn named_values(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < line.len() {
        // the next secret word at or after i
        let hit = SECRET_NAMES.iter().filter_map(|k| lower[i..].find(k).map(|p| (p + i, k.len()))).min();
        let Some((at, klen)) = hit else {
            out.push_str(&line[i..]);
            break;
        };
        out.push_str(&line[i..at + klen]);
        let mut j = at + klen;
        // the rest of the name (password_hash, token-url)
        let rest = &line[j..];
        let tail = rest.find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))).unwrap_or(rest.len());
        out.push_str(&rest[..tail]);
        j += tail;
        // closing quote, spaces, the separator
        let after = &line[j..];
        let q = after.len() - after.trim_start_matches(['"', '\'']).len();
        let sp = after[q..].len() - after[q..].trim_start().len();
        let s0 = q + sp;
        let Some(sep) = after[s0..].chars().next().filter(|c| matches!(c, '=' | ':')) else {
            i = j;
            continue;
        };
        let mut k = s0 + sep.len_utf8();
        let v = &after[k..];
        let sp2 = v.len() - v.trim_start().len();
        k += sp2;
        let v = &after[k..];
        let oq = v.len() - v.trim_start_matches(['"', '\'']).len();
        k += oq;
        let v = &after[k..];
        let end = v.find(is_value_end).unwrap_or(v.len());
        // "password: [removed]" stays; a value of nothing stays
        if end == 0 || &v[..end] == GONE || v[..end].starts_with(GONE) {
            out.push_str(&after[..k]);
            i = j + k;
            continue;
        }
        out.push_str(&after[..k]);
        out.push_str(GONE);
        i = j + k + end;
    }
    out
}

/// A vault entry's name with its value: `vault:openai-key=sk-abc`, `vault entry "x" = "y"`.
fn vault_values(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let Some(at) = lower.find("vault") else { return line.to_string() };
    let rest = &line[at + 5..];
    // a name follows (`vault:name`, `vault.name`, `vault entry name`), then `=` or `:` and a value
    let skip = rest.find(|c: char| !matches!(c, ':' | '.' | ' ' | '"' | '\'' | '/')).unwrap_or(rest.len());
    let name = &rest[skip..];
    let nlen = name.find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))).unwrap_or(name.len());
    if nlen == 0 || skip == 0 {
        return line.to_string();
    }
    let after = &name[nlen..];
    let t = after.trim_start_matches(['"', '\'', ' ']);
    let Some(sep) = t.chars().next().filter(|c| *c == '=') else { return line.to_string() };
    let v = t[sep.len_utf8()..].trim_start_matches([' ', '"', '\'']);
    if v.is_empty() || v.starts_with(GONE) {
        return line.to_string();
    }
    let end = v.find(is_value_end).unwrap_or(v.len());
    let consumed = line.len() - v.len();
    format!("{}{GONE}{}", &line[..consumed], &v[end..])
}

/// Addresses: no `user:password@`, no query string, no fragment.
fn addresses(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(i) = rest.find("://") {
        out.push_str(&rest[..i + 3]);
        let after = &rest[i + 3..];
        let end = after.find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | ')' | ']' | '}')).unwrap_or(after.len());
        let mut tok = &after[..end];
        let mut cred = false;
        let slash = tok.find('/').unwrap_or(tok.len());
        if let Some(a) = tok[..slash].rfind('@') {
            tok = &tok[a + 1..];
            cred = true;
        }
        if cred {
            out.push_str("[credentials]@");
        }
        let cut = tok.find(['?', '#']);
        match cut {
            Some(c) => {
                out.push_str(&tok[..c]);
                out.push_str(if tok.as_bytes()[c] == b'?' { "?***" } else { "#***" });
            }
            None => out.push_str(tok),
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// `Bearer xyz`, `Basic xyz`.
fn schemes(line: &str) -> String {
    let mut out = String::new();
    let mut words = line.split(' ').peekable();
    let mut first = true;
    while let Some(w) = words.next() {
        if !first {
            out.push(' ');
        }
        first = false;
        out.push_str(w);
        if matches!(w.to_ascii_lowercase().as_str(), "bearer" | "basic") {
            if let Some(n) = words.peek() {
                if n.len() >= 8 && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+' | '/' | '=' | '~')) {
                    out.push(' ');
                    out.push_str(GONE);
                    words.next();
                }
            }
        }
    }
    out
}

fn is_hex(c: char) -> bool {
    c.is_ascii_hexdigit()
}

/// MAC addresses `aa:bb:cc:dd:ee:ff` (or with dashes).
fn macs(line: &str) -> String {
    let b: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        if i + 17 <= b.len() && (i == 0 || !b[i - 1].is_ascii_alphanumeric()) && (i + 17 == b.len() || !b[i + 17].is_ascii_alphanumeric()) {
            let w = &b[i..i + 17];
            let sep = w[2];
            if matches!(sep, ':' | '-') && (0..6).all(|k| is_hex(w[k * 3]) && is_hex(w[k * 3 + 1]) && (k == 5 || w[k * 3 + 2] == sep)) {
                out.push_str("[mac]");
                i += 17;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// IPv4 and IPv6 addresses, loopback and the unspecified address excepted.
fn ips(line: &str) -> String {
    let mut out = String::new();
    // split into words by anything that cannot be part of an address, keep the separators
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if word.is_empty() {
            return;
        }
        out.push_str(&ip_word(word));
        word.clear();
    };
    for c in line.chars() {
        if c.is_ascii_hexdigit() || matches!(c, '.' | ':') {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

fn ip_word(w: &str) -> String {
    // trailing punctuation belongs to the sentence, a port to the address
    let core = w.trim_end_matches(['.', ':']);
    let tail = &w[core.len()..];
    let v4 = |s: &str| {
        let p: Vec<&str> = s.split('.').collect();
        p.len() == 4 && p.iter().all(|x| !x.is_empty() && x.len() <= 3 && x.chars().all(|c| c.is_ascii_digit()) && x.parse::<u32>().is_ok_and(|n| n <= 255))
    };
    // 192.0.2.7 or 192.0.2.7:8080
    let (host, port) = match core.rsplit_once(':') {
        Some((h, p)) if v4(h) && !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => (h, format!(":{p}")),
        _ => (core, String::new()),
    };
    if v4(host) {
        if host.starts_with("127.") || host == "0.0.0.0" {
            return w.to_string();
        }
        return format!("[ip]{port}{tail}");
    }
    let colons = core.matches(':').count();
    let ipv6 = colons >= 2 && (core.contains("::") || colons >= 5) && core.chars().all(|c| c.is_ascii_hexdigit() || matches!(c, ':' | '.'));
    if ipv6 && core != "::1" && core != "::" {
        return format!("[ip]{tail}");
    }
    w.to_string()
}

/// `name@domain.tld`.
fn emails(line: &str) -> String {
    let b: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let local_ch = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-');
    let mut i = 0;
    while i < b.len() {
        if b[i] == '@' {
            let mut s = out.chars().count();
            let outc: Vec<char> = out.chars().collect();
            while s > 0 && local_ch(outc[s - 1]) {
                s -= 1;
            }
            let mut e = i + 1;
            while e < b.len() && (b[e].is_ascii_alphanumeric() || matches!(b[e], '.' | '-')) {
                e += 1;
            }
            let dom: String = b[i + 1..e].iter().collect();
            let dom = dom.trim_end_matches(['.', '-']);
            if s < outc.len() && dom.contains('.') && !dom.starts_with('.') {
                let keep: String = outc[..s].iter().collect();
                out = keep;
                out.push_str("[email]");
                i = i + 1 + dom.chars().count();
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// 32 or more hexadecimal characters in a row (keys, hashes of secrets; image checksums go too: they are not needed to read a log).
fn long_hex(line: &str) -> String {
    let mut out = String::new();
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if run.chars().count() >= 32 {
            out.push_str("[hex removed]");
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in line.chars() {
        if is_hex(c) {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// `/home/ann/...` becomes `/home/[user]/...`.
fn homes(line: &str) -> String {
    let mut out = line.to_string();
    for base in ["/home/", "/Users/"] {
        let mut res = String::new();
        let mut rest = out.as_str();
        while let Some(i) = rest.find(base) {
            res.push_str(&rest[..i + base.len()]);
            let after = &rest[i + base.len()..];
            let end = after.find(|c: char| c == '/' || c.is_whitespace() || matches!(c, '"' | '\'' | ':' | ',')).unwrap_or(after.len());
            if end > 0 && &after[..end] != "[user]" {
                res.push_str("[user]");
            } else {
                res.push_str(&after[..end]);
            }
            rest = &after[end..];
        }
        res.push_str(rest);
        out = res;
    }
    out
}

/// The same line with everything secret taken out (see the module text).
pub fn redact_line(line: &str) -> String {
    if let Some(h) = header_values(line) {
        return h;
    }
    let mut t = addresses(line);
    t = vault_values(&t);
    t = named_values(&t);
    t = schemes(&t);
    t = macs(&t);
    t = ips(&t);
    t = emails(&t);
    t = long_hex(&t);
    t = homes(&t);
    // the chat's detector for key formats (`sk-...`, `ghp_...`, JWTs)
    for _ in 0..4 {
        match crate::credguard::detect(&t) {
            Some(f) => t = f.redacted.replace(crate::credguard::PLACEHOLDER, GONE),
            None => break,
        }
    }
    t
}

/// Redact a block of text, line by line; a private key block is cut whole first.
pub fn redact(text: &str) -> String {
    let mut t = text.to_string();
    for _ in 0..3 {
        match t.find("-----BEGIN ") {
            Some(_) => match crate::credguard::detect(&t) {
                Some(f) => t = f.redacted.replace(crate::credguard::PLACEHOLDER, GONE),
                None => break,
            },
            None => break,
        }
    }
    let mut out: Vec<String> = t.lines().map(redact_line).collect();
    // a last look at the whole: nothing the detector knows may be left
    let mut joined = out.join("\n");
    for _ in 0..3 {
        match crate::credguard::detect(&joined) {
            Some(f) => joined = f.redacted.replace(crate::credguard::PLACEHOLDER, GONE),
            None => break,
        }
    }
    out.clear();
    joined
}

// ------------------------------------------------------------------------------------------------ the bundle

/// What the shell collected, as it was (not yet redacted).
#[derive(Clone, Debug, Default)]
pub struct Inputs {
    /// `IYAKKI 0.1.0-alpha.2 · ...` and the release rows.
    pub version: Vec<String>,
    /// One row per service of the startup registry.
    pub services: Vec<String>,
    /// The hardware report's text.
    pub hardware: String,
    /// The doctor's findings (with details).
    pub doctor: Vec<String>,
    /// The kept web-engine log.
    pub engine_log: String,
    /// The end of the boot log.
    pub boot_log: String,
    /// Settings names that are switched on (names only, never values).
    pub switches: Vec<String>,
    /// The newest CHECK-UP report (verdicts and device names; empty when no check-up ran).
    pub checkup: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub title: String,
    pub lines: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bundle {
    pub sections: Vec<Section>,
}

fn section(title: &str, text: &str) -> Section {
    Section { title: title.into(), lines: redact(text).lines().map(str::to_string).collect() }
}

/// Build the bundle: every section redacted.
pub fn build(inp: &Inputs) -> Bundle {
    let mut s = vec![
        section("Version", &inp.version.join("\n")),
        section("Services", &inp.services.join("\n")),
        section("Doctor", &inp.doctor.join("\n")),
        section("Check-up", &inp.checkup.join("\n")),
        section("Switches that are on", &inp.switches.join("\n")),
        section("Hardware", &inp.hardware),
        section("Web engine log (kept lines, no page addresses or content)", &inp.engine_log),
        section("Boot log (end)", &inp.boot_log),
    ];
    // too big: the logs lose their oldest lines first
    let mut guard = 0;
    while text_of(&s).len() > MAX_BYTES && guard < 100_000 {
        guard += 1;
        let Some(big) = s.iter_mut().filter(|x| x.title.contains("log")).max_by_key(|x| x.lines.len()) else { break };
        if big.lines.is_empty() {
            break;
        }
        let drop = (big.lines.len() / 10).max(1);
        big.lines.drain(..drop);
    }
    Bundle { sections: s }
}

fn text_of(s: &[Section]) -> String {
    let mut out = String::new();
    for x in s {
        out.push_str(&format!("=== {} ===\n", x.title));
        if x.lines.is_empty() {
            out.push_str("(nothing)\n");
        }
        for l in &x.lines {
            out.push_str(l);
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

pub const HEADER: &str = "IYAKKI diagnostics. Made on this computer, redacted (secrets, addresses and page content are removed). Nothing was sent anywhere.\n\n";

/// The exact text of the file, which is also what the screen previews.
pub fn preview(b: &Bundle) -> String {
    format!("{HEADER}{}", text_of(&b.sections))
}

/// `iyakki-diagnostics-20261027-181500.txt`
pub fn file_name(unix: i64) -> String {
    let d = unix.div_euclid(86400);
    let s = unix.rem_euclid(86400);
    // civil date from days since 1970 (Howard Hinnant's algorithm)
    let z = d + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("iyakki-diagnostics-{year:04}{month:02}{day:02}-{:02}{:02}{:02}.txt", s / 3600, s % 3600 / 60, s % 60)
}

/// Write exactly [`preview`]'s bytes to `dir/name` (the folder must exist; nothing is replaced).
pub fn save(b: &Bundle, dir: &Path, name: &str) -> Result<PathBuf, String> {
    if name.contains('/') || name.contains("..") || name.is_empty() {
        return Err("that is not a file name".into());
    }
    let p = dir.join(name);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&p).map_err(|e| if e.kind() == std::io::ErrorKind::AlreadyExists { "a file with that name is already there".to_string() } else { format!("cannot write {}: {e}", p.display()) })?;
    use std::io::Write;
    f.write_all(preview(b).as_bytes()).map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())?;
    Ok(p)
}

#[cfg(test)]
#[path = "diagshare_tests.rs"]
mod tests;
