//! Base64 (RFC 4648), standard and URL-safe.

const STD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

pub fn encode(data: &[u8], url: bool, pad: bool) -> String {
    let t = if url { URL } else { STD };
    let mut o = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..=c.len() {
            o.push(t[(n >> (18 - 6 * k)) as usize & 63] as char);
        }
        if pad {
            for _ in c.len()..3 {
                o.push('=');
            }
        }
    }
    o
}

/// Decode either alphabet; white space and missing padding are accepted.
pub fn decode(s: &str) -> Result<Vec<u8>, String> {
    let mut vals = Vec::with_capacity(s.len());
    let mut pads = 0;
    for (i, c) in s.chars().enumerate() {
        let v = match c {
            'A'..='Z' => c as u8 - b'A',
            'a'..='z' => c as u8 - b'a' + 26,
            '0'..='9' => c as u8 - b'0' + 52,
            '+' | '-' => 62,
            '/' | '_' => 63,
            '=' => {
                pads += 1;
                continue;
            }
            c if c.is_whitespace() => continue,
            c => return Err(format!("Not Base64: '{c}' at character {}", i + 1)),
        };
        if pads > 0 {
            return Err("Characters after the '=' padding".into());
        }
        vals.push(v);
    }
    if vals.len() % 4 == 1 {
        return Err("Not Base64: the length is impossible (one character too many or few)".into());
    }
    let mut out = Vec::with_capacity(vals.len() * 3 / 4);
    for c in vals.chunks(4) {
        let n = c.iter().enumerate().fold(0u32, |a, (k, v)| a | (*v as u32) << (18 - 6 * k));
        for k in 0..c.len() - 1 {
            out.push((n >> (16 - 8 * k)) as u8);
        }
    }
    Ok(out)
}

pub fn encode_text(input: &str, url: bool) -> Result<String, String> {
    Ok(encode(input.as_bytes(), url, !url))
}

/// Decode to text; bytes that are not UTF-8 are shown as a hex dump instead.
pub fn decode_text(input: &str) -> Result<String, String> {
    let b = decode(input)?;
    Ok(match String::from_utf8(b) {
        Ok(t) => t,
        Err(e) => {
            let b = e.into_bytes();
            let hex: Vec<String> = b.iter().take(4096).map(|x| format!("{x:02x}")).collect();
            format!("Not text ({} bytes). Hex:\n{}{}", b.len(), hex.join(" "), if b.len() > 4096 { " ..." } else { "" })
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc_4648_vectors() {
        for (p, e) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(encode_text(p, false).unwrap(), e);
            assert_eq!(decode_text(e).unwrap(), p);
        }
    }

    #[test]
    fn url_safe_has_no_plus_slash_or_padding_and_both_decode() {
        let data = [0xfbu8, 0xff, 0xfe, 0x3e];
        assert_eq!(encode(&data, false, true), "+//+Pg==");
        assert_eq!(encode(&data, true, false), "-__-Pg");
        assert_eq!(decode("+//+Pg==").unwrap(), data);
        assert_eq!(decode("-__-Pg").unwrap(), data);
        assert_eq!(decode("Zm9v\nYmFy ").unwrap(), b"foobar");
    }

    #[test]
    fn bad_input_and_binary_are_reported() {
        assert!(decode("Zm9v!").unwrap_err().contains("'!'"));
        assert!(decode("Z").is_err());
        assert!(decode_text("//8=").unwrap().starts_with("Not text (2 bytes)"));
    }
}
