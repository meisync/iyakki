//! SPDX licence expressions (`MIT OR Apache-2.0`, `(A OR B) AND C`, `X WITH Y`) evaluated against an allow list, and the looser
//! comma lists Buildroot writes (`GPL-2.0+, BSD-3-Clause (libuuid)`) classified for an operating-system image.

/// The licences a Rust dependency may carry without a review. Comparison ignores case.
pub const CARGO_ALLOW: &[&str] = &[
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "MIT",
    "MIT-0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Zlib",
    "Unicode-3.0",
    "Unicode-DFS-2016",
    "CC0-1.0",
    "0BSD",
    "Unlicense",
    "BSL-1.0",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Id(String),
    And(Vec<Expr>),
    Or(Vec<Expr>),
}

fn tokens(s: &str) -> Vec<String> {
    // the old crates.io spelling `MIT/Apache-2.0` means OR
    let s = s.replace('/', " OR ").replace('(', " ( ").replace(')', " ) ");
    s.split_whitespace().map(str::to_string).collect()
}

pub fn parse(s: &str) -> Result<Expr, String> {
    let t = tokens(s);
    if t.is_empty() {
        return Err("empty licence expression".into());
    }
    let (e, i) = parse_or(&t, 0)?;
    if i != t.len() {
        return Err(format!("unexpected `{}` in `{s}`", t[i]));
    }
    Ok(e)
}

fn parse_or(t: &[String], mut i: usize) -> Result<(Expr, usize), String> {
    let (first, j) = parse_and(t, i)?;
    i = j;
    let mut v = vec![first];
    while i < t.len() && t[i].eq_ignore_ascii_case("or") {
        let (e, j) = parse_and(t, i + 1)?;
        v.push(e);
        i = j;
    }
    Ok((if v.len() == 1 { v.pop().unwrap() } else { Expr::Or(v) }, i))
}

fn parse_and(t: &[String], mut i: usize) -> Result<(Expr, usize), String> {
    let (first, j) = parse_term(t, i)?;
    i = j;
    let mut v = vec![first];
    while i < t.len() && t[i].eq_ignore_ascii_case("and") {
        let (e, j) = parse_term(t, i + 1)?;
        v.push(e);
        i = j;
    }
    Ok((if v.len() == 1 { v.pop().unwrap() } else { Expr::And(v) }, i))
}

fn parse_term(t: &[String], i: usize) -> Result<(Expr, usize), String> {
    match t.get(i).map(String::as_str) {
        None => Err("expression ends early".into()),
        Some("(") => {
            let (e, j) = parse_or(t, i + 1)?;
            if t.get(j).map(String::as_str) != Some(")") {
                return Err("missing `)`".into());
            }
            Ok((e, j + 1))
        }
        Some(")") => Err("unexpected `)`".into()),
        Some(id) if id.eq_ignore_ascii_case("or") || id.eq_ignore_ascii_case("and") || id.eq_ignore_ascii_case("with") => Err(format!("unexpected `{id}`")),
        Some(id) => {
            if t.get(i + 1).is_some_and(|w| w.eq_ignore_ascii_case("with")) {
                let ex = t.get(i + 2).ok_or("`WITH` without an exception")?;
                return Ok((Expr::Id(format!("{id} WITH {ex}")), i + 3));
            }
            Ok((Expr::Id(id.to_string()), i + 1))
        }
    }
}

/// Is the expression satisfiable using only `allow`? On failure, the identifiers that block it.
pub fn satisfied(e: &Expr, allow: &[&str]) -> Result<(), Vec<String>> {
    match e {
        Expr::Id(id) => {
            if allow.iter().any(|a| a.eq_ignore_ascii_case(id)) {
                Ok(())
            } else {
                Err(vec![id.clone()])
            }
        }
        Expr::And(v) => {
            let bad: Vec<String> = v.iter().filter_map(|x| satisfied(x, allow).err()).flatten().collect();
            if bad.is_empty() { Ok(()) } else { Err(bad) }
        }
        Expr::Or(v) => {
            let mut bad = Vec::new();
            for x in v {
                match satisfied(x, allow) {
                    Ok(()) => return Ok(()),
                    Err(b) => bad.extend(b),
                }
            }
            Err(bad)
        }
    }
}

/// How an operating-system component's licence reads for the image scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// Permissive or public domain.
    Permissive,
    /// Copyleft: allowed for separate programs and libraries, with notices and a source offer (`legal-info/sources`).
    Copyleft,
    /// A vendor licence (`Proprietary`, `redistributable`, `nonfree`): needs a recorded owner decision.
    Proprietary,
    /// Not stated, or a name this gate does not know yet: needs a review.
    Unknown,
}

fn split_commas(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut depth) = (Vec::new(), String::new(), 0i32);
    for c in s.chars() {
        match c {
            '(' => {
                depth += 1;
                cur.push(c)
            }
            ')' => {
                depth -= 1;
                cur.push(c)
            }
            ',' if depth == 0 => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter().map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()
}

const IMAGE_PERMISSIVE: &[&str] = &[
    "mit", "bsd", "isc", "zlib", "apache", "public domain", "cc0", "0bsd", "unlicense", "wtfpl", "openssl", "curl", "libpng", "ijg", "bzip2", "ofl", "bitstream", "dejavu", "hpnd", "icu", "unicode", "freetype", "ftl", "x11", "libtiff", "python", "psf", "blueoak", "sgi", "boost", "bsl", "ngpl", "zpl", "lppl", "xfree86", "liberation", "ubuntu-font", "cdla", "artistic", "libjpeg", "ilmbase", "openexr", "pil", "tcl", "mit-like", "ntp", "bsd-like", "info-zip", "ssleay", "ffmpeg-build", "sleepycat", "gd", "zlib-acknowledgement", "wxwindows", "ucd", "afl", "fontconfig", "ncsa", "blessing", "khronos",
];
const IMAGE_COPYLEFT: &[&str] = &["gpl", "lgpl", "agpl", "mpl", "epl", "cddl", "gcc-exception", "eupl", "cecill", "osl"];

/// Classify one token of a Buildroot licence list (`GPL-2.0+ (aserver)`).
pub fn classify_token(tok: &str) -> Class {
    let head = tok.split('(').next().unwrap_or("").trim().to_lowercase();
    if head.is_empty() {
        return Class::Unknown;
    }
    if head.contains("proprietary") || head.contains("nonfree") || head.contains("non-free") || head.contains("redistributable") || head.contains("commercial") {
        return Class::Proprietary;
    }
    if head == "unknown" || head.contains("sspl") || head.contains("elastic") {
        return if head.contains("sspl") || head.contains("elastic") { Class::Proprietary } else { Class::Unknown };
    }
    if IMAGE_COPYLEFT.iter().any(|k| head.starts_with(k)) {
        return Class::Copyleft;
    }
    if IMAGE_PERMISSIVE.iter().any(|k| head.starts_with(k)) {
        return Class::Permissive;
    }
    Class::Unknown
}

/// The worst class among the tokens of a Buildroot licence string, and the tokens that decide it.
pub fn classify_list(license: &str) -> (Class, Vec<String>) {
    let toks = split_commas(license);
    if toks.is_empty() {
        return (Class::Unknown, vec!["(none)".into()]);
    }
    // Buildroot writes alternatives as `A or B`: the licensee may pick the better one
    let class_of = |t: &String| {
        let low = t.to_lowercase();
        if low.contains(" or ") && !low.contains('(') { low.split(" or ").map(|a| classify_token(a)).min().unwrap_or(Class::Unknown) } else if let Some((a, rest)) = low.split_once(" or ") { classify_token(a).min(classify_token(rest.split('(').next().unwrap_or(rest))) } else { classify_token(t) }
    };
    let worst = toks.iter().map(class_of).max().unwrap();
    // Copyleft is the worst a permissive-or-copyleft list may get; Proprietary / Unknown dominate
    let deciding = toks.iter().filter(|t| class_of(t) == worst).cloned().collect();
    (worst, deciding)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expressions() {
        let ok = |s: &str| satisfied(&parse(s).unwrap(), CARGO_ALLOW).is_ok();
        assert!(ok("MIT OR Apache-2.0") && ok("MIT/Apache-2.0") && ok("(MIT OR Apache-2.0) AND Unicode-3.0") && ok("Apache-2.0 WITH LLVM-exception"));
        assert!(ok("MIT OR LGPL-3.0-or-later") && ok("Apache-2.0 OR GPL-2.0-only"));
        assert!(!ok("GPL-3.0") && !ok("MIT AND GPL-2.0-only") && !ok("MPL-2.0") && !ok("Apache-2.0 WITH Classpath-exception-2.0") && !ok("LicenseRef-Proprietary"));
        assert_eq!(satisfied(&parse("MIT AND OFL-1.1 AND Ubuntu-font-1.0").unwrap(), CARGO_ALLOW), Err(vec!["OFL-1.1".into(), "Ubuntu-font-1.0".into()]));
        assert!(parse("").is_err() && parse("MIT OR").is_err() && parse("(MIT").is_err() && parse("MIT AND OR GPL").is_err());
    }

    #[test]
    fn image_licence_lists() {
        assert_eq!(classify_list("MIT").0, Class::Permissive);
        assert_eq!(classify_list("LGPL-2.1+ (library), GPL-2.0+ (aserver)").0, Class::Copyleft);
        assert_eq!(classify_list("GPL-2.0, bzip2-1.0.4").0, Class::Copyleft);
        assert_eq!(classify_list("MIT, BSD-2-Clause, Public domain, Unlicense, WTFPL").0, Class::Permissive);
        assert_eq!(classify_list("Proprietary").0, Class::Proprietary);
        assert_eq!(classify_list("GPL-2.0, Proprietary").0, Class::Proprietary);
        assert_eq!(classify_list("something-new").0, Class::Unknown);
        assert_eq!(classify_list("unknown").0, Class::Unknown);
        assert_eq!(classify_list("").0, Class::Unknown);
        assert_eq!(classify_list("AFL-2.1 or GPL-2.0+ (library, tools), GPL-2.0+ (tools)").0, Class::Copyleft);
        assert_eq!(classify_list("FTL or GPL-2.0+").0, Class::Permissive);
        assert_eq!(classify_list("LGPL-2.0+, GPL-2.0+, BSD-2-Clause, UNKNOWN").0, Class::Unknown);
        assert_eq!(classify_list("MIT, SGI, Khronos").0, Class::Permissive);
    }
}
