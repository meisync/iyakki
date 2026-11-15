//! The first gate for every imported file (threat list, "Odd input"): size, lines, UTF-8, control characters.

use crate::error::VpnError;

pub const MAX_BYTES: usize = 256 * 1024;
pub const MAX_LINES: usize = 4000;
pub const MAX_LINE: usize = 8 * 1024;
/// One inline block (`<ca>` ...) may not exceed this.
pub const MAX_BLOCK: usize = 64 * 1024;
pub const MAX_NAME: usize = 40;

/// Check the raw bytes and return the text with CRLF turned into LF and a leading BOM removed.
pub fn clean(input: &[u8]) -> Result<String, VpnError> {
    if input.len() > MAX_BYTES {
        return Err(VpnError::TooLarge(format!("the profile is larger than {} KB", MAX_BYTES / 1024)));
    }
    let s = std::str::from_utf8(input).map_err(|_| VpnError::BadText("the profile is not valid UTF-8 text".into()))?;
    let s = s.strip_prefix('\u{feff}').unwrap_or(s);
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    continue; // the \n follows
                }
                return Err(VpnError::BadText("the profile contains a lone carriage return".into()));
            }
            '\n' | '\t' => out.push(c),
            c if c.is_control() => {
                return Err(VpnError::BadText(format!("the profile contains a control character (U+{:04X})", c as u32)));
            }
            c => out.push(c),
        }
    }
    let mut lines = 0usize;
    for l in out.split('\n') {
        lines += 1;
        if l.len() > MAX_LINE {
            return Err(VpnError::TooLarge(format!("line {lines} is longer than {} KB", MAX_LINE / 1024)));
        }
    }
    if lines > MAX_LINES {
        return Err(VpnError::TooLarge(format!("the profile has more than {MAX_LINES} lines")));
    }
    Ok(out)
}

/// A profile name: 1 to 40 printable characters. Names are labels only, never file names.
pub fn check_name(name: &str) -> Result<String, VpnError> {
    let n = name.trim();
    let count = n.chars().count();
    if count == 0 || count > MAX_NAME {
        return Err(VpnError::BadText(format!("the name must be 1 to {MAX_NAME} characters")));
    }
    if n.chars().any(|c| c.is_control()) {
        return Err(VpnError::BadText("the name contains a control character".into()));
    }
    Ok(n.to_string())
}

/// Host names as used for endpoints: letters, digits, dots, hyphens, at most 253 bytes, labels of 1 to 63.
pub fn valid_hostname(h: &str) -> bool {
    if h.is_empty() || h.len() > 253 {
        return false;
    }
    h.trim_end_matches('.').split('.').all(|l| {
        !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}
