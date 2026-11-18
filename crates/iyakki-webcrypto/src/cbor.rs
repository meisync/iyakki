//! The small part of CBOR (RFC 8949) that WebAuthn and CTAP2 use: integers, byte and text strings, arrays, maps, booleans and null.
//! The encoder is canonical in the CTAP2 sense (shortest forms, map keys sorted by encoded length then bytes). The decoder takes
//! definite lengths only and refuses what it does not know, with a depth and size limit.

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Int(i64),
    Bytes(Vec<u8>),
    Text(String),
    Array(Vec<Value>),
    Map(Vec<(Value, Value)>),
    Bool(bool),
    Null,
}

impl Value {
    pub fn text(s: &str) -> Value {
        Value::Text(s.to_string())
    }
    pub fn get(&self, key: &Value) -> Option<&Value> {
        match self {
            Value::Map(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn get_text(&self, key: &str) -> Option<&Value> {
        self.get(&Value::text(key))
    }
    pub fn as_bytes(&self) -> Option<&[u8]> {
        if let Value::Bytes(b) = self { Some(b) } else { None }
    }
    pub fn as_int(&self) -> Option<i64> {
        if let Value::Int(i) = self { Some(*i) } else { None }
    }
}

fn head(out: &mut Vec<u8>, major: u8, n: u64) {
    let m = major << 5;
    match n {
        0..=23 => out.push(m | n as u8),
        24..=0xff => out.extend_from_slice(&[m | 24, n as u8]),
        0x100..=0xffff => {
            out.push(m | 25);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        0x1_0000..=0xffff_ffff => {
            out.push(m | 26);
            out.extend_from_slice(&(n as u32).to_be_bytes());
        }
        _ => {
            out.push(m | 27);
            out.extend_from_slice(&n.to_be_bytes());
        }
    }
}

pub fn encode(v: &Value) -> Vec<u8> {
    let mut o = Vec::new();
    enc(v, &mut o);
    o
}

fn enc(v: &Value, o: &mut Vec<u8>) {
    match v {
        Value::Int(i) if *i >= 0 => head(o, 0, *i as u64),
        Value::Int(i) => head(o, 1, (-1 - *i) as u64),
        Value::Bytes(b) => {
            head(o, 2, b.len() as u64);
            o.extend_from_slice(b);
        }
        Value::Text(s) => {
            head(o, 3, s.len() as u64);
            o.extend_from_slice(s.as_bytes());
        }
        Value::Array(a) => {
            head(o, 4, a.len() as u64);
            a.iter().for_each(|x| enc(x, o));
        }
        Value::Map(m) => {
            let mut rows: Vec<(Vec<u8>, Vec<u8>)> = m.iter().map(|(k, v)| (encode(k), encode(v))).collect();
            rows.sort_by(|a, b| a.0.len().cmp(&b.0.len()).then_with(|| a.0.cmp(&b.0)));
            head(o, 5, rows.len() as u64);
            for (k, v) in rows {
                o.extend_from_slice(&k);
                o.extend_from_slice(&v);
            }
        }
        Value::Bool(b) => o.push(if *b { 0xf5 } else { 0xf4 }),
        Value::Null => o.push(0xf6),
    }
}

/// Decodes one value and returns it with the number of bytes used.
pub fn decode(b: &[u8]) -> Result<(Value, usize), String> {
    dec(b, 0, 0)
}

/// Decodes a whole input: anything after the value is an error.
pub fn decode_all(b: &[u8]) -> Result<Value, String> {
    let (v, n) = decode(b)?;
    if n != b.len() {
        return Err("bytes after the CBOR value".into());
    }
    Ok(v)
}

fn dec(b: &[u8], at: usize, depth: usize) -> Result<(Value, usize), String> {
    if depth > 16 {
        return Err("CBOR nested too deep".into());
    }
    let first = *b.get(at).ok_or("CBOR ends early")?;
    let (major, info) = (first >> 5, first & 31);
    let mut p = at + 1;
    let n: u64 = match info {
        0..=23 => info as u64,
        24..=27 => {
            let len = 1usize << (info - 24);
            let s = b.get(p..p + len).ok_or("CBOR ends early")?;
            p += len;
            s.iter().fold(0u64, |a, x| a << 8 | *x as u64)
        }
        _ => return Err("indefinite or reserved CBOR lengths are not accepted".into()),
    };
    match major {
        0 => Ok((Value::Int(i64::try_from(n).map_err(|_| "integer too big")?), p - at)),
        1 => Ok((Value::Int(-1 - i64::try_from(n).map_err(|_| "integer too big")?), p - at)),
        2 | 3 => {
            let end = p.checked_add(usize::try_from(n).map_err(|_| "too long")?).ok_or("too long")?;
            let s = b.get(p..end).ok_or("CBOR ends early")?;
            let v = if major == 2 { Value::Bytes(s.to_vec()) } else { Value::Text(String::from_utf8(s.to_vec()).map_err(|_| "text is not UTF-8")?) };
            Ok((v, end - at))
        }
        4 | 5 => {
            if n > 4096 {
                return Err("CBOR container too big".into());
            }
            let mut items = Vec::new();
            for _ in 0..(if major == 4 { n } else { n * 2 }) {
                let (v, used) = dec(b, p, depth + 1)?;
                p += used;
                items.push(v);
            }
            if major == 4 {
                return Ok((Value::Array(items), p - at));
            }
            let mut it = items.into_iter();
            let mut m = Vec::new();
            while let (Some(k), Some(v)) = (it.next(), it.next()) {
                m.push((k, v));
            }
            Ok((Value::Map(m), p - at))
        }
        7 => match info {
            20 => Ok((Value::Bool(false), 1)),
            21 => Ok((Value::Bool(true), 1)),
            22 => Ok((Value::Null, 1)),
            _ => Err("CBOR simple value not accepted".into()),
        },
        _ => Err("CBOR tag not accepted".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn h(v: &Value) -> String {
        encode(v).iter().map(|x| format!("{x:02x}")).collect()
    }
    #[test]
    fn rfc8949_appendix_a_examples() {
        for (v, hex) in [
            (Value::Int(0), "00"),
            (Value::Int(23), "17"),
            (Value::Int(24), "1818"),
            (Value::Int(100), "1864"),
            (Value::Int(1000), "1903e8"),
            (Value::Int(1_000_000), "1a000f4240"),
            (Value::Int(-1), "20"),
            (Value::Int(-100), "3863"),
            (Value::Int(-1000), "3903e7"),
            (Value::Bytes(vec![1, 2, 3, 4]), "4401020304"),
            (Value::text("IETF"), "6449455446"),
            (Value::Array(vec![Value::Int(1), Value::Array(vec![Value::Int(2), Value::Int(3)])]), "8201820203"),
            (Value::Bool(true), "f5"),
            (Value::Null, "f6"),
            (Value::Map(vec![(Value::Int(1), Value::Int(2)), (Value::Int(3), Value::Int(4))]), "a201020304"),
        ] {
            assert_eq!(h(&v), hex);
            assert_eq!(decode_all(&encode(&v)).unwrap(), v);
        }
    }
    #[test]
    fn maps_are_sorted_the_ctap2_way_and_bad_input_is_refused() {
        let m = Value::Map(vec![(Value::text("authData"), Value::Int(1)), (Value::text("fmt"), Value::Int(2)), (Value::text("attStmt"), Value::Int(3)), (Value::Int(-1), Value::Int(0)), (Value::Int(3), Value::Int(0))]);
        let d = decode_all(&encode(&m)).unwrap();
        let Value::Map(rows) = d else { panic!() };
        let keys: Vec<_> = rows.iter().map(|(k, _)| k.clone()).collect();
        assert_eq!(keys, vec![Value::Int(3), Value::Int(-1), Value::text("fmt"), Value::text("attStmt"), Value::text("authData")]);
        for bad in [&[0x5f][..], &[0x44, 1, 2][..], &[0xc1, 0][..], &[0x18][..], &[0x00, 0x00][..], &[0xf9, 0, 0][..]] {
            assert!(decode_all(bad).is_err(), "{bad:?}");
        }
        assert!(decode_all(&[0x81; 40]).is_err(), "depth");
    }
}
