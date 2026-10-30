//! Standard base64 (RFC 4648) for the 32-byte WireGuard keys. Our own, small, strict: padding required, no whitespace.

const ALPHA: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(ALPHA[(n >> 18) as usize & 63] as char);
        out.push(ALPHA[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { ALPHA[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { ALPHA[n as usize & 63] as char } else { '=' });
    }
    out
}

pub fn decode(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if b.len() % 4 != 0 {
        return None;
    }
    let val = |c: u8| ALPHA.iter().position(|&a| a == c).map(|p| p as u32);
    let mut out = Vec::with_capacity(b.len() / 4 * 3);
    for (i, q) in b.chunks(4).enumerate() {
        let last = (i + 1) * 4 == b.len();
        let pad = q.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for (j, &c) in q.iter().enumerate() {
            n <<= 6;
            if j < 4 - pad {
                n |= val(c)?;
            } else if c != b'=' {
                return None;
            }
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
        // the unused low bits of the last group must be zero (canonical form)
        if pad == 2 && n & 0xffff != 0 || pad == 1 && n & 0xff != 0 {
            return None;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc4648_vectors() {
        for (raw, enc) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(encode(raw.as_bytes()), enc);
            assert_eq!(decode(enc).unwrap(), raw.as_bytes());
        }
    }

    #[test]
    fn bad_input_is_refused() {
        for bad in ["Zg=", "Zg", "Z===", "Zm9v Zg==", "Zm=v", "Zm9v\n", "Zh==", "Zm9=", "=Zm9"] {
            assert!(decode(bad).is_none(), "{bad:?}");
        }
    }
}
