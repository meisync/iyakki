//! Inventory of everything IYAKKI downloads or pulls at the person's click (Quick install items, command-line tools, model weights,
//! store catalogue entries): `legal/downloads.txt`. Row: `id | licence | delivery | pin kind | pin | reason | date`.
//!   * delivery is `download-only` (fetched or pulled by the person's click, never part of an image or an export) or
//!     `nothing-installed` (IYAKKI installs nothing); anything else, `shipped` in particular, fails.
//!   * pin kind is `digest` (a `sha256:` hex digest), `commit` (a pinned source commit with per-file hashes), `version` (an exact version or tag), `checksum` (the vendor's published checksum
//!     is verified at download time but the version floats: listed as REVIEW), `none-vendor` (an exact version is pinned in the code but the vendor publishes no checksum: listed as REVIEW, and the item's screen shows a warning), `floating` (no pin at all: listed as REVIEW) or `n/a`
//!     (nothing is downloaded).
//! `licence` is the SPDX expression of the thing downloaded, or a plain statement for vendor licences (`LicenseRef-...`).
//! Tests in `iyakki-agent` (`tests/licence_gate.rs`) tie this file to the real Quick install list and the built-in store catalogue.

use crate::spdx::{self, Class};
use crate::table;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: String,
    pub license: String,
    pub delivery: String,
    pub pin_kind: String,
    pub pin: String,
    pub reason: String,
    pub date: String,
    pub line: usize,
}

pub fn parse(text: &str) -> (Vec<Row>, Vec<String>) {
    let (mut v, mut problems) = (Vec::new(), Vec::new());
    for r in table::parse(text) {
        if r.f.len() != 7 {
            problems.push(format!("downloads line {}: expected `id | licence | delivery | pin kind | pin | reason | date`", r.line));
            continue;
        }
        let f = &r.f;
        v.push(Row { id: f[0].clone(), license: f[1].clone(), delivery: f[2].clone(), pin_kind: f[3].clone(), pin: f[4].clone(), reason: f[5].clone(), date: f[6].clone(), line: r.line });
    }
    (v, problems)
}

#[derive(Debug, Default)]
pub struct Report {
    pub failures: Vec<String>,
    pub review: Vec<String>,
    pub rows: usize,
}

fn hex64(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

pub fn check(rows: &[Row], parse_problems: Vec<String>) -> Report {
    let mut r = Report { failures: parse_problems, rows: rows.len(), ..Default::default() };
    let mut ids = std::collections::BTreeSet::new();
    for x in rows {
        let at = format!("downloads line {} ({})", x.line, x.id);
        if !ids.insert(x.id.clone()) {
            r.failures.push(format!("{at}: duplicate id"));
        }
        if x.license.is_empty() || x.license.eq_ignore_ascii_case("unknown") || x.license == "-" {
            r.failures.push(format!("{at}: no licence stated"));
        } else if spdx::classify_list(&x.license.replace(" OR ", ", ").replace(" AND ", ", ")).0 == Class::Unknown && !x.license.contains("LicenseRef-") {
            r.failures.push(format!("{at}: licence `{}` is not recognised (use an SPDX id or LicenseRef-...)", x.license));
        }
        if !matches!(x.delivery.as_str(), "download-only" | "nothing-installed") {
            r.failures.push(format!("{at}: delivery `{}` must be download-only or nothing-installed (nothing here may be shipped in an image)", x.delivery));
        }
        if !table::valid_reason(&x.reason) || !table::valid_date(&x.date) {
            r.failures.push(format!("{at}: needs a reason (what was checked) and a date YYYY-MM-DD"));
        }
        match x.pin_kind.as_str() {
            "digest" if !x.pin.strip_prefix("sha256:").is_some_and(hex64) => r.failures.push(format!("{at}: digest pin must be sha256:<64 hex>")),
            "version" if x.pin.is_empty() || x.pin.eq_ignore_ascii_case("latest") => r.failures.push(format!("{at}: version pin missing")),
            "none-vendor" if x.pin.is_empty() || x.pin.eq_ignore_ascii_case("latest") => r.failures.push(format!("{at}: `none-vendor` needs the pinned version")),
            "none-vendor" => r.review.push(format!("{at}: version pinned ({}), but the vendor publishes no checksum, so the download cannot be proven to be the vendor's", x.pin)),
            "digest" | "version" | "n/a" => {}
            "commit" if x.pin.is_empty() => r.failures.push(format!("{at}: commit pin missing")),
            "commit" => {}
            "checksum" => r.review.push(format!("{at}: version floats, only the vendor's checksum is verified at download ({})", x.pin)),
            "floating" => r.review.push(format!("{at}: NO version or digest pin ({})", x.pin)),
            other => r.failures.push(format!("{at}: pin kind `{other}` unknown")),
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: &str = "sha256:f238979f3c80da6f383f2e4fe91be4dc3580d7ee3b7edec83d0b8783c53b74a4";

    #[test]
    fn good_rows_pass_and_floating_ones_are_listed() {
        let t = format!("graph-db | GPL-3.0-only | download-only | digest | {D} | Neo4j Community, pulled by the cluster, LICENSE read | 2026-10-26\nnode | MIT | download-only | checksum | vendor SHASUMS256 | vendor checksum file verified at download | 2026-10-26\nclaude | LicenseRef-Anthropic-commercial | download-only | floating | vendor installer | proprietary CLI, downloaded at the click | 2026-10-26\nmac-host | LicenseRef-none | nothing-installed | n/a | - | nothing is installed by IYAKKI | 2026-10-26\n");
        let (rows, p) = parse(&t);
        let r = check(&rows, p);
        assert!(r.failures.is_empty(), "{:?}", r.failures);
        assert_eq!(r.review.len(), 2);
    }

    #[test]
    fn missing_licence_shipped_delivery_and_missing_pin_fail() {
        let t = "a | - | download-only | version | 1.0 | fine reason for this row | 2026-10-26\nb | MIT | shipped | version | 1.0 | fine reason for this row | 2026-10-26\nc | MIT | download-only | digest | sha256:abc | fine reason for this row | 2026-10-26\nd | MIT | download-only | version | latest | fine reason for this row | 2026-10-26\ne | MIT | download-only | version | 1 | x | 2026-10-26\ne | totally-new-licence | download-only | version | 1 | fine reason for this row | 2026-10-26\nshort | row\n";
        let (rows, p) = parse(t);
        let r = check(&rows, p);
        assert_eq!(r.failures.len(), 8, "{:#?}", r.failures);
    }
}
