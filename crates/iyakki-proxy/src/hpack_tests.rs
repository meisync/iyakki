use super::*;

fn unhex(s: &str) -> Vec<u8> {
    let s: String = s.split_whitespace().collect();
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
}

fn h(n: &str, v: &str) -> Header {
    (n.to_string(), v.to_string())
}

#[test]
fn the_huffman_code_is_complete_and_the_rfc_examples_encode_and_decode() {
    // the lengths fill the code space exactly (Kraft sum 1 with the 30-bit end-of-string symbol): no code is a prefix of another, no gap
    let sum: u128 = CODES.iter().map(|c| 1u128 << (30 - c.1 as u32)).sum::<u128>() + 1;
    assert_eq!(sum, 1u128 << 30);
    // RFC 7541 C.4: the strings of the request examples
    for (text, hex) in [("www.example.com", "f1e3 c2e5 f23a 6ba0 ab90 f4ff"), ("no-cache", "a8eb 1064 9cbf"), ("custom-key", "25a8 49e9 5ba9 7d7f"), ("custom-value", "25a8 49e9 5bb8 e8b4 bf")] {
        assert_eq!(huffman_encode(text.as_bytes()), unhex(hex), "{text}");
        assert_eq!(huffman_decode(&unhex(hex)).unwrap(), text.as_bytes());
        assert_eq!(huffman_len(text.as_bytes()), unhex(hex).len());
    }
    // every byte value goes there and back, alone and in a row
    let all: Vec<u8> = (0..=255).collect();
    assert_eq!(huffman_decode(&huffman_encode(&all)).unwrap(), all);
    for b in 0..=255u8 {
        assert_eq!(huffman_decode(&huffman_encode(&[b, b, b])).unwrap(), vec![b, b, b]);
    }
    assert_eq!(huffman_decode(&[]).unwrap(), Vec::<u8>::new());
}

#[test]
fn bad_huffman_strings_are_refused_not_guessed_at() {
    assert_eq!(huffman_decode(&[0xff, 0xff, 0xff, 0xff]), Err(HpackError::BadHuffman), "padding longer than seven bits, or the end-of-string symbol");
    // a valid code followed by padding that is not ones: 'a' is 00011, then 000 as padding
    assert_eq!(huffman_decode(&[0b0001_1000]), Err(HpackError::BadHuffman));
    // 'a' followed by the right padding (111) is fine
    assert_eq!(huffman_decode(&[0b0001_1111]).unwrap(), b"a");
    assert_eq!(huffman_decode(&huffman_encode(&[0x00; 3000])).unwrap().len(), 3000);
}

#[test]
fn integers_follow_the_rfc_examples_and_refuse_overflow() {
    // C.1.1: 10 in a 5-bit prefix; C.1.2: 1337 in a 5-bit prefix; C.1.3: 42 at the start of an octet
    let mut o = Vec::new();
    encode_int(&mut o, 0, 5, 10);
    assert_eq!(o, [0b0000_1010]);
    o.clear();
    encode_int(&mut o, 0, 5, 1337);
    assert_eq!(o, [0b0001_1111, 0b1001_1010, 0b0000_1010]);
    assert_eq!(decode_int(&o, 0, 5).unwrap(), (1337, 3));
    o.clear();
    encode_int(&mut o, 0, 8, 42);
    assert_eq!(o, [42]);
    for v in [0usize, 1, 30, 31, 32, 127, 128, 255, 16383, 16384, 2_000_000] {
        for prefix in 1..=8u8 {
            let mut e = Vec::new();
            encode_int(&mut e, 0, prefix, v);
            assert_eq!(decode_int(&e, 0, prefix).unwrap(), (v, e.len()), "{v} in {prefix}");
        }
    }
    assert_eq!(decode_int(&[0x1f, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f], 0, 5), Err(HpackError::IntegerOverflow));
    assert_eq!(decode_int(&[0x1f, 0x80], 0, 5), Err(HpackError::Truncated));
}

#[test]
fn the_rfc_request_examples_with_huffman_decode_with_a_growing_table_and_the_encoder_makes_the_same_bytes() {
    let blocks = [
        ("8286 8441 8cf1 e3c2 e5f2 3a6b a0ab 90f4 ff", vec![h(":method", "GET"), h(":scheme", "http"), h(":path", "/"), h(":authority", "www.example.com")]),
        ("8286 84be 5886 a8eb 1064 9cbf", vec![h(":method", "GET"), h(":scheme", "http"), h(":path", "/"), h(":authority", "www.example.com"), h("cache-control", "no-cache")]),
        ("8287 85bf 4088 25a8 49e9 5ba9 7d7f 8925 a849 e95b b8e8 b4bf", vec![h(":method", "GET"), h(":scheme", "https"), h(":path", "/index.html"), h(":authority", "www.example.com"), h("custom-key", "custom-value")]),
    ];
    let (mut dec, mut enc) = (Decoder::default(), Encoder::default());
    for (hex, want) in blocks {
        assert_eq!(dec.decode(&unhex(hex)).unwrap(), want, "{hex}");
        assert_eq!(enc.encode(&want), unhex(hex), "the encoder writes what the RFC prints for {hex}");
    }
    // the decoder's table after the three: custom-key, :authority, cache-control in the order the RFC lists (C.4.3: 3 entries, 164 bytes)
    assert_eq!(dec.table.entries.len(), 3);
    assert_eq!(dec.table.size, 164);
    assert_eq!(dec.table.get(62), Some(h("custom-key", "custom-value")));
}

#[test]
fn literals_without_huffman_and_the_never_indexed_names_and_the_size_rules() {
    // C.2.1 literal with indexing, new name: custom-key: custom-header (no Huffman)
    let mut d = Decoder::default();
    let b = unhex("400a 6375 7374 6f6d 2d6b 6579 0d63 7573 746f 6d2d 6865 6164 6572");
    assert_eq!(d.decode(&b).unwrap(), vec![h("custom-key", "custom-header")]);
    assert_eq!(d.table.size, 55);
    // C.2.2 literal without indexing, indexed name: :path: /sample/path; nothing is added to the table
    let b = unhex("040c 2f73 616d 706c 652f 7061 7468");
    assert_eq!(d.decode(&b).unwrap(), vec![h(":path", "/sample/path")]);
    assert_eq!(d.table.entries.len(), 1);
    // C.2.3 never indexed: password: secret
    let b = unhex("1008 7061 7373 776f 7264 0673 6563 7265 74");
    assert_eq!(d.decode(&b).unwrap(), vec![h("password", "secret")]);
    assert_eq!(d.table.entries.len(), 1);
    // the encoder never lets a cookie or a credential into a table, and says so with the never-indexed form
    let mut e = Encoder::default();
    let out = e.encode(&[h("authorization", "Bearer abc"), h("cookie", "a=b"), h("x-trace", "1")]);
    assert_eq!(out[0] & 0xf0, 0x10, "authorization is a never-indexed literal");
    assert_eq!(e.table.entries.len(), 1, "only x-trace was kept");
    let mut back = Decoder::default();
    assert_eq!(back.decode(&out).unwrap(), vec![h("authorization", "Bearer abc"), h("cookie", "a=b"), h("x-trace", "1")]);
    // a size change comes first, never above what was allowed, and shrinks the table
    let mut d = Decoder::new(100);
    assert_eq!(d.decode(&[0x3f, 0xe1, 0x1f]), Err(HpackError::BadSizeUpdate), "4096 is over the allowed 100");
    assert_eq!(d.decode(&[0x82, 0x20]), Err(HpackError::BadSizeUpdate), "after a header it is no longer allowed");
    d.decode(&unhex("400a 6375 7374 6f6d 2d6b 6579 0d63 7573 746f 6d2d 6865 6164 6572")).unwrap();
    assert_eq!(d.table.entries.len(), 1, "55 bytes fit in a table of 100");
    // an entry bigger than the whole table empties it instead of being kept
    let mut e = Encoder::default();
    let big = e.encode(&[h("x-big", &"v".repeat(120))]);
    d.decode(&big).unwrap();
    assert_eq!(d.table.entries.len(), 0);
}

#[test]
fn the_dynamic_table_evicts_the_oldest_and_the_encoder_announces_a_lowered_limit() {
    let mut d = Decoder::new(120);
    for i in 0..10 {
        let mut e = Encoder::default();
        let b = e.encode(&[h("x-n", &format!("value-{i}"))]);
        d.decode(&b).unwrap();
    }
    assert!(d.table.size <= 120 && d.table.entries.len() <= 3, "{} entries, {} bytes", d.table.entries.len(), d.table.size);
    assert_eq!(d.table.entries.front().unwrap().1, "value-9", "the newest is first");
    let mut e = Encoder::default();
    e.encode(&[h("x-a", "1")]);
    e.set_peer_limit(0);
    let b = e.encode(&[h("x-b", "2")]);
    assert_eq!(b[0], 0x20, "a size change to 0 starts the next block");
    assert_eq!(e.table.entries.len(), 0, "a table of 0 bytes keeps nothing: the old entry is gone and x-b is not kept");
}

#[test]
fn hostile_blocks_are_errors_not_panics_and_sizes_are_capped() {
    let mut d = Decoder::default();
    assert_eq!(d.decode(&[0xff, 0x7f]), Err(HpackError::BadIndex(254)));
    assert_eq!(d.decode(&[0x80]), Err(HpackError::BadIndex(0)));
    assert_eq!(d.decode(&[0x40, 0x05, b'a']), Err(HpackError::Truncated));
    // a string that claims to be a gigabyte
    assert_eq!(d.decode(&[0x40, 0x7f, 0xff, 0xff, 0xff, 0x07]), Err(HpackError::TooLarge));
    // the whole list over its limit: many 60 KB values
    let mut big = Vec::new();
    for _ in 0..6 {
        big.push(0x00);
        big.push(1);
        big.push(b'x');
        encode_int(&mut big, 0, 7, 60_000);
        big.extend(std::iter::repeat(b'v').take(60_000));
    }
    assert_eq!(Decoder::default().decode(&big), Err(HpackError::TooLarge));
    // random bytes never panic
    let mut x = 0x1234_5678u32;
    for _ in 0..3000 {
        let len = (x % 40) as usize;
        let v: Vec<u8> = (0..len).map(|_| { x = x.wrapping_mul(1664525).wrapping_add(1013904223); (x >> 24) as u8 }).collect();
        let _ = Decoder::default().decode(&v);
        let _ = huffman_decode(&v);
    }
    assert_eq!(STATIC_TABLE.len(), 61);
    assert_eq!(STATIC_TABLE[60].0, "www-authenticate");
}
