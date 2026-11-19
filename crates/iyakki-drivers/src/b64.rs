//! Base64 (standard alphabet), in-house: firmware blobs travel inside the JSON payload of a driver pack.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { ALPHABET[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { ALPHABET[n as usize & 63] as char } else { '=' });
    }
    out
}

pub fn decode(s: &str) -> Option<Vec<u8>> {
    let s: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if s.len() % 4 != 0 {
        return None;
    }
    let val = |b: u8| ALPHABET.iter().position(|a| *a == b).map(|p| p as u32);
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    for (i, q) in s.chunks(4).enumerate() {
        let last = i == s.len() / 4 - 1;
        let pad = q.iter().rev().take_while(|b| **b == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for (j, b) in q.iter().enumerate() {
            n = n << 6 | if j >= 4 - pad { 0 } else { val(*b)? };
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_length_and_rejects_garbage() {
        for n in 0..40usize {
            let d: Vec<u8> = (0..n).map(|i| (i * 7 + 3) as u8).collect();
            assert_eq!(decode(&encode(&d)).unwrap(), d, "{n}");
        }
        assert_eq!(encode(b"Man"), "TWFu");
        assert_eq!(encode(b"Ma"), "TWE=");
        assert!(decode("TWE").is_none() && decode("T!Fu").is_none() && decode("TW==TWFu").is_none());
    }
}
