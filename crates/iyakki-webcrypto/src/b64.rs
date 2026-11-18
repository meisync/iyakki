//! Base64, standard and URL-safe, with and without padding (RFC 4648).

const STD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn enc(data: &[u8], t: &[u8; 64], pad: bool) -> String {
    let mut o = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                o.push(t[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else if pad {
                o.push('=');
            }
        }
    }
    o
}

/// Decodes either alphabet; padding is optional; anything else is refused.
pub fn decode(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim_end_matches('=');
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for ch in s.bytes() {
        let v = match ch {
            b'A'..=b'Z' => ch - b'A',
            b'a'..=b'z' => ch - b'a' + 26,
            b'0'..=b'9' => ch - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return Err("not base64".into()),
        } as u32;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    if s.len() % 4 == 1 {
        return Err("not base64".into());
    }
    Ok(out)
}

pub fn url(data: &[u8]) -> String {
    enc(data, URL, false)
}
pub fn std(data: &[u8]) -> String {
    enc(data, STD, true)
}
pub fn std_nopad(data: &[u8]) -> String {
    enc(data, STD, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rfc4648_vectors_in_both_alphabets() {
        for (p, e) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(std(p.as_bytes()), e);
            assert_eq!(decode(e).unwrap(), p.as_bytes());
            assert_eq!(url(p.as_bytes()), e.trim_end_matches('='));
            assert_eq!(decode(&url(p.as_bytes())).unwrap(), p.as_bytes());
        }
        assert_eq!(url(&[0xfb, 0xff]), "-_8");
        assert_eq!(std(&[0xfb, 0xff]), "+/8=");
        assert!(decode("a$b").is_err() && decode("a").is_err());
    }
}
