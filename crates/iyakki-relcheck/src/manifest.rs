//! The downloads manifest as a gate of the release (`legal/downloads-manifest.json`, docs/development/DOWNLOADS-MANIFEST.md).
//! The signed-data rules live in `iyakki-agent::dlmanifest`; this file is the gate's own cross-check, with no dependency on the agent:
//! the manifest is read with a small JSON reader of its own and tied to the rows of `legal/downloads.txt`.
//!
//!   * every REVIEW row (pin kind `checksum`, `none-vendor`, `floating`) has an entry (its id, or one of its `covers`), and every entry
//!     is a row of the list: neither side can drift from the other;
//!   * a `checksum` row is a `float+sum` item whose checksum is the vendor's file (address on https); a `none-vendor` row is a
//!     `none-vendor` item pinned to the row's version; anything else is `no-pin`;
//!   * the licence of an entry is the row's; nothing is `shipped`; ids are unique;
//!   * a `none-vendor` item without a SHA-256 of our own is a REVIEW line (the download cannot be proven to be the vendor's);
//!   * the public key file is `unset` (REVIEW: no fetched manifest is accepted) or 64 hex characters, never a private key.
//! The signature is not checked here (the gate has no key material): `iyakki dlmanifest verify` and the agent tests do that.

use crate::downloads::Row;

#[derive(Debug, Clone, PartialEq)]
pub enum J {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

impl J {
    pub fn get(&self, k: &str) -> Option<&J> {
        match self {
            J::Obj(v) => v.iter().find(|(n, _)| n == k).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn str(&self) -> Option<&str> {
        match self {
            J::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn arr(&self) -> &[J] {
        match self {
            J::Arr(v) => v,
            _ => &[],
        }
    }
}

struct P<'a> {
    b: &'a [u8],
    i: usize,
    depth: usize,
}

pub fn parse_json(text: &str) -> Result<J, String> {
    let mut p = P { b: text.as_bytes(), i: 0, depth: 0 };
    let v = p.value()?;
    p.ws();
    if p.i != p.b.len() {
        return Err(format!("unexpected text after the JSON value at byte {}", p.i));
    }
    Ok(v)
}

impl P<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\n' | b'\r' | b'\t') {
            self.i += 1;
        }
    }
    fn eat(&mut self, c: u8) -> Result<(), String> {
        self.ws();
        if self.b.get(self.i) == Some(&c) {
            self.i += 1;
            Ok(())
        } else {
            Err(format!("expected `{}` at byte {}", c as char, self.i))
        }
    }
    fn lit(&mut self, word: &str, v: J) -> Result<J, String> {
        if self.b[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(v)
        } else {
            Err(format!("unexpected text at byte {}", self.i))
        }
    }
    fn value(&mut self) -> Result<J, String> {
        self.ws();
        self.depth += 1;
        if self.depth > 32 {
            return Err("nested too deeply".into());
        }
        let r = match self.b.get(self.i) {
            None => Err("the JSON ends too early".into()),
            Some(b'{') => {
                self.i += 1;
                let mut v = Vec::new();
                self.ws();
                if self.b.get(self.i) == Some(&b'}') {
                    self.i += 1;
                } else {
                    loop {
                        self.ws();
                        let k = self.string()?;
                        self.eat(b':')?;
                        v.push((k, self.value()?));
                        self.ws();
                        match self.b.get(self.i) {
                            Some(b',') => self.i += 1,
                            Some(b'}') => {
                                self.i += 1;
                                break;
                            }
                            _ => return Err(format!("expected `,` or `}}` at byte {}", self.i)),
                        }
                    }
                }
                Ok(J::Obj(v))
            }
            Some(b'[') => {
                self.i += 1;
                let mut v = Vec::new();
                self.ws();
                if self.b.get(self.i) == Some(&b']') {
                    self.i += 1;
                } else {
                    loop {
                        v.push(self.value()?);
                        self.ws();
                        match self.b.get(self.i) {
                            Some(b',') => self.i += 1,
                            Some(b']') => {
                                self.i += 1;
                                break;
                            }
                            _ => return Err(format!("expected `,` or `]` at byte {}", self.i)),
                        }
                    }
                }
                Ok(J::Arr(v))
            }
            Some(b'"') => self.string().map(J::Str),
            Some(b't') => self.lit("true", J::Bool(true)),
            Some(b'f') => self.lit("false", J::Bool(false)),
            Some(b'n') => self.lit("null", J::Null),
            Some(c) if c.is_ascii_digit() || *c == b'-' => {
                let s = self.i;
                while self.i < self.b.len() && matches!(self.b[self.i], b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E') {
                    self.i += 1;
                }
                std::str::from_utf8(&self.b[s..self.i]).ok().and_then(|t| t.parse::<f64>().ok()).map(J::Num).ok_or_else(|| format!("not a number at byte {s}"))
            }
            Some(_) => Err(format!("unexpected character at byte {}", self.i)),
        };
        self.depth -= 1;
        r
    }
    fn string(&mut self) -> Result<String, String> {
        if self.b.get(self.i) != Some(&b'"') {
            return Err(format!("expected a string at byte {}", self.i));
        }
        self.i += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let c = *self.b.get(self.i).ok_or("a string is not closed")?;
            self.i += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    let e = *self.b.get(self.i).ok_or("a string is not closed")?;
                    self.i += 1;
                    match e {
                        b'"' | b'\\' | b'/' => out.push(e),
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'b' | b'f' => out.push(b' '),
                        b'u' => {
                            let h = self.b.get(self.i..self.i + 4).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u32::from_str_radix(h, 16).ok()).ok_or("a \\u escape is not 4 hex digits")?;
                            self.i += 4;
                            let ch = char::from_u32(h).unwrap_or('\u{fffd}');
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        }
                        _ => return Err("unknown escape in a string".into()),
                    }
                }
                c if c < 0x20 => return Err("a control character inside a string".into()),
                c => out.push(c),
            }
        }
        String::from_utf8(out).map_err(|_| "a string is not UTF-8".to_string())
    }
}

#[derive(Debug, Default)]
pub struct Report {
    pub failures: Vec<String>,
    pub review: Vec<String>,
    pub items: usize,
    pub sequence: u64,
}

fn hex(s: &str, n: usize) -> bool {
    s.len() == n && s.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

/// `Some(true)` = a usable public key, `Some(false)` = looks wrong, `None` = `unset`.
fn key_state(text: &str) -> Option<bool> {
    let v = text.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#'))?;
    if v == "unset" {
        return None;
    }
    Some(hex(v, 64))
}

pub fn check(manifest: &str, rows: &[Row], pubkey_file: &str) -> Report {
    let mut r = Report::default();
    match key_state(pubkey_file) {
        None => r.review.push("manifest key: legal/downloads-manifest.pub says `unset`, so no fetched manifest is accepted (fail closed) until the owner makes the key".into()),
        Some(false) => r.failures.push("manifest key: legal/downloads-manifest.pub is neither `unset` nor 64 lower-case hex characters".into()),
        Some(true) => {}
    }
    let j = match parse_json(manifest) {
        Ok(j) => j,
        Err(e) => {
            r.failures.push(format!("manifest: legal/downloads-manifest.json is not valid JSON: {e}"));
            return r;
        }
    };
    if !matches!(j.get("schema"), Some(J::Num(n)) if *n == 1.0) {
        r.failures.push("manifest: schema must be 1".into());
    }
    match j.get("sequence") {
        Some(J::Num(n)) if *n >= 1.0 && n.fract() == 0.0 => r.sequence = *n as u64,
        _ => r.failures.push("manifest: sequence must be a whole number from 1".into()),
    }
    if !j.get("issued").and_then(J::str).is_some_and(crate::table::valid_date) {
        r.failures.push("manifest: issued must be a date YYYY-MM-DD".into());
    }
    let items = j.get("items").map_or(&[][..], J::arr);
    r.items = items.len();
    if items.is_empty() {
        r.failures.push("manifest: no items".into());
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut covered = std::collections::BTreeMap::new();
    for it in items {
        let id = it.get("id").and_then(J::str).unwrap_or("");
        let at = format!("manifest item `{id}`");
        if id.is_empty() {
            r.failures.push("manifest: an item has no id".into());
            continue;
        }
        let covers: Vec<&str> = it.get("covers").map_or(&[][..], J::arr).iter().filter_map(J::str).collect();
        for c in std::iter::once(id).chain(covers.iter().copied()) {
            if !ids.insert(c.to_string()) {
                r.failures.push(format!("{at}: `{c}` appears twice"));
            }
            covered.insert(c.to_string(), it);
        }
        if it.get("downloaded_not_shipped") != Some(&J::Bool(true)) {
            r.failures.push(format!("{at}: downloaded_not_shipped must be true (nothing in the list may be shipped)"));
        }
        let kind = it.get("kind").and_then(J::str).unwrap_or("");
        let sum = it.get("checksum");
        let sum_type = sum.and_then(|c| c.get("type")).and_then(J::str).unwrap_or("");
        match (kind, sum_type) {
            ("float+sum", "vendor-file") => {
                let u = sum.and_then(|c| c.get("url")).and_then(J::str).unwrap_or("");
                if !u.starts_with("https://") {
                    r.failures.push(format!("{at}: the vendor checksum file must be on an https address"));
                }
            }
            ("none-vendor", "") => r.review.push(format!("{at}: pinned to `{}` but no SHA-256 of our own is recorded, so a download cannot be proven to be the vendor's (`iyakki dlmanifest pin`)", it.get("version").and_then(J::str).unwrap_or(""))),
            ("none-vendor", "sha256") => {
                let files = sum.and_then(|c| c.get("files")).map_or(&[][..], J::arr);
                if files.is_empty() || files.iter().any(|f| !f.get("sha256").and_then(J::str).is_some_and(|h| hex(h, 64))) {
                    r.failures.push(format!("{at}: a sha256 checksum needs files with 64 hex characters each"));
                }
            }
            ("no-pin", "") => {}
            (k, s) => r.failures.push(format!("{at}: kind `{k}` does not go with checksum `{}`", if s.is_empty() { "(none)" } else { s })),
        }
        if kind == "float+sum" && it.get("version").and_then(J::str).is_some_and(|v| v.is_empty()) {
            r.failures.push(format!("{at}: an empty version is written by leaving the field out"));
        }
    }
    let by_id: std::collections::BTreeMap<&str, &Row> = rows.iter().map(|x| (x.id.as_str(), x)).collect();
    for x in rows.iter().filter(|x| matches!(x.pin_kind.as_str(), "checksum" | "none-vendor" | "floating")) {
        let at = format!("downloads line {} ({})", x.line, x.id);
        let Some(it) = covered.get(&x.id) else {
            r.failures.push(format!("{at}: a REVIEW row with no entry in legal/downloads-manifest.json (add it, then run the ignored agent test `regenerate_the_bundled_file`)"));
            continue;
        };
        let kind = it.get("kind").and_then(J::str).unwrap_or("");
        let sum_type = it.get("checksum").and_then(|c| c.get("type")).and_then(J::str).unwrap_or("");
        if it.get("id").and_then(J::str) == Some(x.id.as_str()) && it.get("licence").and_then(J::str) != Some(x.license.as_str()) {
            r.failures.push(format!("{at}: the manifest names another licence than the row"));
        }
        match x.pin_kind.as_str() {
            "checksum" if kind != "float+sum" || sum_type != "vendor-file" => r.failures.push(format!("{at}: a checksum row needs a `float+sum` entry with the vendor's checksum file, the manifest has `{kind}`")),
            "none-vendor" if kind != "none-vendor" || it.get("version").and_then(J::str) != Some(x.pin.as_str()) => r.failures.push(format!("{at}: a none-vendor row needs a `none-vendor` entry pinned to `{}`", x.pin)),
            "floating" if kind != "no-pin" => r.failures.push(format!("{at}: a floating row is `no-pin` in the manifest, it has `{kind}`")),
            _ => {}
        }
    }
    for id in &ids {
        if !by_id.contains_key(id.as_str()) {
            r.failures.push(format!("manifest item `{id}` is not a row of legal/downloads.txt"));
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::downloads::parse;

    const ROWS: &str = "\
cli:kubectl | Apache-2.0 | download-only | checksum | dl.k8s.io kubectl.sha256 | vendor checksum file verified at download | 2026-10-27
cli:aws-cli | Apache-2.0 | download-only | none-vendor | 2.38.6 | pinned, vendor publishes no checksum | 2026-10-27
mac-firmware | LicenseRef-Apple-firmware-no-redistribution | download-only | floating | Apple recovery image | person's own Mac, hashes per file in the table | 2026-10-26
k3s | Apache-2.0 | download-only | checksum | GitHub sha256sum | release checksum file verified at download | 2026-10-26
cli:k3s | Apache-2.0 | download-only | checksum | GitHub sha256sum-<arch>.txt | licence read: the repository LICENSE file | 2026-10-27
";
    const KEY: &str = "# key\n5eb1dfe1d8eeb40b2b1aace60ce6272700b391c8a942ab13c47f12c506c2972d\n";

    fn good() -> String {
        r#"{"schema":1,"sequence":3,"issued":"2026-10-28","items":[
 {"id":"cli:kubectl","licence":"Apache-2.0","proprietary":false,"downloaded_not_shipped":true,"kind":"float+sum","checksum":{"type":"vendor-file","url":"https://dl.k8s.io/x.sha256","format":"single"}},
 {"id":"cli:aws-cli","licence":"Apache-2.0","proprietary":false,"downloaded_not_shipped":true,"kind":"none-vendor","version":"2.38.6","checksum":{"type":"sha256","files":[{"file":"a.zip","sha256":"0000000000000000000000000000000000000000000000000000000000000000"}],"date":"2026-11-01","source":"s"}},
 {"id":"mac-firmware","licence":"LicenseRef-Apple-firmware-no-redistribution","proprietary":true,"downloaded_not_shipped":true,"kind":"no-pin"},
 {"id":"cli:k3s","covers":["k3s"],"licence":"Apache-2.0","proprietary":false,"downloaded_not_shipped":true,"kind":"float+sum","checksum":{"type":"vendor-file","url":"https://github.com/k/sha","format":"lines"}}
]}"#
            .to_string()
    }

    fn run(m: &str, key: &str) -> Report {
        let (rows, p) = parse(ROWS);
        assert!(p.is_empty());
        check(m, &rows, key)
    }

    #[test]
    fn a_manifest_that_matches_the_rows_passes_and_an_unset_key_is_a_review_line() {
        let r = run(&good(), KEY);
        assert!(r.failures.is_empty() && r.review.is_empty(), "{r:?}");
        assert_eq!((r.items, r.sequence), (4, 3));
        let r = run(&good(), "# c\nunset\n");
        assert!(r.failures.is_empty() && r.review.len() == 1 && r.review[0].contains("fail closed"), "{r:?}");
        assert!(!run(&good(), "PRIVATE-KEY-text").failures.is_empty());
    }

    #[test]
    fn a_row_without_an_entry_a_stray_entry_and_wrong_kinds_all_fail() {
        let missing = good().replace("\"id\":\"cli:aws-cli\"", "\"id\":\"cli:other\"");
        let r = run(&missing, KEY);
        assert!(r.failures.iter().any(|f| f.contains("cli:aws-cli") && f.contains("no entry")), "{r:?}");
        assert!(r.failures.iter().any(|f| f.contains("cli:other") && f.contains("not a row")), "{r:?}");
        let no_sum = good().replace("\"kind\":\"float+sum\",\"checksum\":{\"type\":\"vendor-file\",\"url\":\"https://dl.k8s.io/x.sha256\",\"format\":\"single\"}", "\"kind\":\"no-pin\"");
        assert!(run(&no_sum, KEY).failures.iter().any(|f| f.contains("cli:kubectl") && f.contains("vendor's checksum file")));
        let wrong_pin = good().replace("\"version\":\"2.38.6\"", "\"version\":\"2.0.0\"");
        assert!(run(&wrong_pin, KEY).failures.iter().any(|f| f.contains("pinned to `2.38.6`")));
        let shipped = good().replace("\"downloaded_not_shipped\":true,\"kind\":\"no-pin\"", "\"downloaded_not_shipped\":false,\"kind\":\"no-pin\"");
        assert!(run(&shipped, KEY).failures.iter().any(|f| f.contains("mac-firmware") && f.contains("shipped")));
        let lic = good().replace("\"licence\":\"Apache-2.0\",\"proprietary\":false,\"downloaded_not_shipped\":true,\"kind\":\"float+sum\",\"checksum\":{\"type\":\"vendor-file\",\"url\":\"https://dl.k8s", "\"licence\":\"MIT\",\"proprietary\":false,\"downloaded_not_shipped\":true,\"kind\":\"float+sum\",\"checksum\":{\"type\":\"vendor-file\",\"url\":\"https://dl.k8s");
        assert!(run(&lic, KEY).failures.iter().any(|f| f.contains("another licence")));
        let http = good().replace("https://dl.k8s.io/x.sha256", "http://dl.k8s.io/x.sha256");
        assert!(run(&http, KEY).failures.iter().any(|f| f.contains("https")));
        let dup = good().replace("\"covers\":[\"k3s\"]", "\"covers\":[\"cli:kubectl\"]");
        assert!(run(&dup, KEY).failures.iter().any(|f| f.contains("appears twice")));
    }

    #[test]
    fn a_none_vendor_item_without_our_own_sha256_is_listed_for_review_and_bad_json_fails() {
        let bare = good().replace(",\"checksum\":{\"type\":\"sha256\",\"files\":[{\"file\":\"a.zip\",\"sha256\":\"0000000000000000000000000000000000000000000000000000000000000000\"}],\"date\":\"2026-11-01\",\"source\":\"s\"}", "");
        let r = run(&bare, KEY);
        assert!(r.failures.is_empty() && r.review.len() == 1 && r.review[0].contains("cli:aws-cli"), "{r:?}");
        for bad in ["", "{", "{\"schema\":1,}", "[1,2", "{\"a\":\"\\q\"}", "nul", "{} x"] {
            assert!(run(bad, KEY).failures.iter().any(|f| f.contains("not valid JSON")), "{bad}");
        }
        let weak = good().replace("\"schema\":1", "\"schema\":2").replace("\"sequence\":3", "\"sequence\":0");
        let r = run(&weak, KEY);
        assert!(r.failures.iter().any(|f| f.contains("schema")) && r.failures.iter().any(|f| f.contains("sequence")), "{r:?}");
    }

    #[test]
    fn the_json_reader_reads_strings_escapes_numbers_and_nesting() {
        let j = parse_json(r#" {"a":[1,-2.5,true,null,"x\u00e9\n\"q\""],"b":{"c":{}}} "#).unwrap();
        assert_eq!(j.get("a").unwrap().arr().len(), 5);
        assert_eq!(j.get("a").unwrap().arr()[4].str(), Some("x\u{e9}\n\"q\""));
        assert_eq!(j.get("b").unwrap().get("c"), Some(&J::Obj(vec![])));
        assert!(parse_json(&format!("{}{}", "[".repeat(40), "]".repeat(40))).is_err(), "deep nesting is refused");
    }
}
