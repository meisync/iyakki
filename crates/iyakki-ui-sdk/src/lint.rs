//! A scan for styling numbers written into widget or app code (F-SDK-121). Colours, radii, borders, gaps, shadow sizes, glyph sizes and
//! font sizes come from the skin's tokens (`Metrics::tokens`, `Metrics::gap`, `Metrics::size`); a number of 3.0 or more written in a
//! drawing function is almost always one of those, so a test of every widget file and every SDK app runs [`style_literals`] on its
//! source. What it lets through: numbers under 3 (halves, doubles, ratios), a line that declares a `const` (a named proportion), the
//! argument of `em(` (a width in text sizes), comments, strings and everything from the first `#[cfg(test)]` on.

/// The lines of `src` (1-based, with the text) that hold a decimal number of 3.0 or more outside the places listed above.
pub fn style_literals(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, raw) in src.lines().enumerate() {
        let t = raw.trim_start();
        if t.starts_with("#[cfg(test)]") {
            break;
        }
        if t.starts_with("//") || t.starts_with("const ") || t.starts_with("pub const ") {
            continue;
        }
        let code = strip(raw);
        let code = code.replace("em(", "em_(");
        let b = code.as_bytes();
        let mut k = 0;
        while k < b.len() {
            if b[k].is_ascii_digit() && (k == 0 || !(b[k - 1].is_ascii_alphanumeric() || b[k - 1] == b'_' || b[k - 1] == b'.')) {
                let start = k;
                while k < b.len() && b[k].is_ascii_digit() {
                    k += 1;
                }
                if k + 1 < b.len() && b[k] == b'.' && b[k + 1].is_ascii_digit() {
                    let int: f32 = code[start..k].parse().unwrap_or(0.0);
                    // the argument of `em(`: skip
                    let in_em = start >= 4 && &code[start - 4..start] == "em_(";
                    if int >= 3.0 && !in_em {
                        out.push(format!("line {}: {}", i + 1, raw.trim()));
                        break;
                    }
                }
            } else {
                k += 1;
            }
        }
    }
    out
}

/// The line without a trailing `//` comment and without the inside of string literals.
fn strip(line: &str) -> String {
    let mut s = String::new();
    let mut in_str = false;
    let mut prev = ' ';
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if in_str {
            if c == '"' && prev != '\\' {
                in_str = false;
            }
            prev = c;
            continue;
        }
        if c == '"' {
            in_str = true;
        } else if c == '/' && chars.peek() == Some(&'/') {
            break;
        } else {
            s.push(c);
        }
        prev = c;
    }
    s
}

/// An app that registers for events must handle the `missed` marker and look at `late` (docs/sdk/events.md). Returns what it lacks, from the
/// app's source (everything before the first `#[cfg(test)]`): empty when the app does not use events or handles both.
pub fn events_unhandled(src: &str) -> Vec<&'static str> {
    let body = src.split("#[cfg(test)]").next().unwrap_or("");
    let code: String = body.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
    if !(code.contains("EventRouter") || code.contains("EventsPort")) || !(code.contains(".on(") || code.contains(".on_prefix(") || code.contains(".register(")) {
        return Vec::new();
    }
    let mut out = Vec::new();
    if !code.contains("on_missed(") && !code.contains("is_missed()") {
        out.push("missed");
    }
    if !code.contains(".late") {
        out.push("late");
    }
    if !code.contains("on_permissions(") {
        out.push("permissions");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::style_literals;

    #[test]
    fn an_app_that_registers_for_events_must_handle_missed_and_late() {
        use super::events_unhandled;
        assert!(events_unhandled("fn a() { let x = 1; }").is_empty());
        assert_eq!(events_unhandled("let r = EventRouter::new().on(\"x\", |e, a| {});"), ["missed", "late", "permissions"]);
        assert_eq!(events_unhandled("let r = EventRouter::new().on(\"x\", |e, a| { if e.late {} }).on_permissions(|c, e, a| {});"), ["missed"]);
        assert!(events_unhandled("let r = EventRouter::new().on(\"x\", |e, a| { if e.late {} }).on_missed(|_, _| {}).on_permissions(|c, e, a| {});").is_empty());
        assert!(events_unhandled("// EventRouter .on(\nfn a() {}").is_empty());
    }

    #[test]
    fn the_scan_finds_styling_numbers_and_lets_proportions_through() {
        assert_eq!(style_literals("c.rrect(r, 8.0, col);").len(), 1);
        assert_eq!(style_literals("let s = m.font(15.0);").len(), 1);
        assert_eq!(style_literals("r.inset(12.0)").len(), 1);
        assert!(style_literals("let half = r.w / 2.0 * 1.35;").is_empty());
        assert!(style_literals("const SEND_EM: f32 = 5.0;").is_empty());
        assert!(style_literals("m.em(10.0).min(w)").is_empty());
        assert!(style_literals("// a gap of 12.0 here\nlet x = \"1280.0 wide\";").is_empty());
        assert!(style_literals("fn a() {}\n#[cfg(test)]\nmod t { fn b() { f(99.0); } }").is_empty());
        assert_eq!(style_literals("x\ny + 6.5").len(), 1);
    }
}
