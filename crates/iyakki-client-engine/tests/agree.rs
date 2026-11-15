//! The engine's decoder is a copy of the instance's (`iyakki_link::pixelcodec`, without PNG tiles). These tests encode real pictures with the
//! instance's encoder, in every level, and check that both decoders give the same pixels, and that bad data is an error, never a panic.

use iyakki_client_engine::picture;
use iyakki_link::pixelcodec::{self as pc, Caps, Level, Opts, Streamer};

fn picture_of(w: usize, h: usize) -> Vec<u8> {
    let mut v = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            let px: [u8; 3] = if x < w / 4 {
                [12, 24, 48] // solid background
            } else if x < w / 2 {
                if (x / 3 + y / 5) % 2 == 0 { [250, 250, 250] } else { [20, 20, 20] } // text-like, two colours
            } else if x < 3 * w / 4 {
                [(x * 7 % 256) as u8, (y * 5 % 256) as u8, ((x * y) % 251) as u8] // photo-like, many colours
            } else {
                [(x % 4 * 60) as u8, (y % 3 * 90) as u8, 128] // a few colours
            };
            v[i..i + 3].copy_from_slice(&px);
            v[i + 3] = 255;
        }
    }
    v
}

#[test]
fn both_decoders_give_the_same_picture_for_every_level_and_size() {
    for level in Level::ALL {
        for (w, h) in [(64, 64), (200, 130), (257, 65)] {
            let mut st = Streamer::new();
            st.set_source(w, h, picture_of(w, h));
            let mut ours = picture::Canvas::default();
            let mut theirs = pc::Canvas::default();
            let mut n = 0;
            // a few rounds: the first holds the picture, later ones refine lossy tiles
            for round in 0..4u64 {
                let o = Opts { level, caps: Caps { deflate: true }, budget: usize::MAX, now_ms: round * 1000 };
                let Some(e) = st.encode(&o) else { continue };
                let a = picture::decode_frame(&e.msg, &mut ours).unwrap_or_else(|x| panic!("{level:?} {w}x{h}: {x:?}"));
                let b = pc::decode_frame(&e.msg, &mut theirs).unwrap();
                assert_eq!((a.seq, a.screen_w, a.screen_h, a.scale, a.level, a.tiles), (b.seq, b.screen_w, b.screen_h, b.scale, b.level as u8, b.tiles));
                assert_eq!((ours.w, ours.h), (theirs.w, theirs.h));
                assert_eq!(ours.rgba, theirs.rgba, "{level:?} {w}x{h} round {round}");
                n += 1;
            }
            assert!(n >= 1, "at least the first picture was sent");
        }
    }
}

#[test]
fn a_lossless_picture_comes_back_exactly() {
    let (w, h) = (130, 70);
    let src = picture_of(w, h);
    let mut st = Streamer::new();
    st.set_source(w, h, src.clone());
    let e = st.encode(&Opts { level: Level::Lossless, caps: Caps { deflate: true }, budget: usize::MAX, now_ms: 0 }).unwrap();
    let mut c = picture::Canvas::default();
    picture::decode_frame(&e.msg, &mut c).unwrap();
    assert_eq!(c.rgba, src);
}

#[test]
fn a_changed_region_is_drawn_over_the_old_picture() {
    let (w, h) = (128, 128);
    let mut src = picture_of(w, h);
    let mut st = Streamer::new();
    st.set_source(w, h, src.clone());
    let o = |t| Opts { level: Level::Lossless, caps: Caps { deflate: true }, budget: usize::MAX, now_ms: t };
    let mut c = picture::Canvas::default();
    picture::decode_frame(&st.encode(&o(0)).unwrap().msg, &mut c).unwrap();
    for y in 70..90 {
        for x in 70..90 {
            src[(y * w + x) * 4..(y * w + x) * 4 + 3].copy_from_slice(&[1, 2, 3]);
        }
    }
    st.set_source(w, h, src.clone());
    let e = st.encode(&o(100)).unwrap();
    assert!(e.tiles < 4, "only the tile that changed is sent");
    picture::decode_frame(&e.msg, &mut c).unwrap();
    assert_eq!(c.rgba, src);
}

#[test]
fn png_tiles_are_refused_and_bad_data_is_an_error_not_a_panic() {
    // a client that cannot inflate gets PNG tiles from the encoder; this engine always says it can, and refuses them
    let (w, h) = (64, 64);
    let mut st = Streamer::new();
    st.set_source(w, h, picture_of(w, h));
    let e = st.encode(&Opts { level: Level::Lossless, caps: Caps { deflate: false }, budget: usize::MAX, now_ms: 0 }).unwrap();
    let mut c = picture::Canvas::default();
    let r = picture::decode_frame(&e.msg, &mut c);
    // a picture made only of solid tiles has no PNG tile; this one has more than one colour per tile
    assert!(matches!(r, Err(picture::DecodeError::Tile(_))), "{r:?}");

    let good = {
        let mut st = Streamer::new();
        st.set_source(100, 100, picture_of(100, 100));
        st.encode(&Opts { level: Level::Balanced, caps: Caps { deflate: true }, budget: usize::MAX, now_ms: 0 }).unwrap().msg
    };
    let mut c = picture::Canvas::default();
    assert!(picture::decode_frame(&[], &mut c).is_err());
    assert!(picture::decode_frame(&good[..10], &mut c).is_err());
    for cut in [18, 19, good.len() / 2, good.len() - 1] {
        let _ = picture::decode_frame(&good[..cut], &mut c);
    }
    // flip every byte in turn: never a panic
    for i in 0..good.len().min(400) {
        let mut bad = good.clone();
        bad[i] ^= 0xff;
        let _ = picture::decode_frame(&bad, &mut c);
    }
    // a size far over the limit
    let mut huge = good.clone();
    huge[6..8].copy_from_slice(&60000u16.to_le_bytes());
    assert_eq!(picture::decode_frame(&huge, &mut c).unwrap_err(), picture::DecodeError::Size);
}

// ------------------------------------------------------------------------------------------------ a frame shared with the node test

/// `tests/golden/frame.bin` is a frame made by the instance's encoder (balanced level: exact tiles and lossy ones); `frame.json` has the
/// SHA-256 of the pixels the instance's decoder gives. Both the engine here and the engine in WebAssembly (`js/test/glue.test.mjs`) must give
/// those pixels. To remake the files: `IYAKKI_GOLDEN_WRITE=1 cargo test -p iyakki-client-engine --test agree golden`.
fn golden_frame() -> Vec<u8> {
    let (w, h) = (150, 90);
    let mut st = Streamer::new();
    st.set_source(w, h, picture_of(w, h));
    st.encode(&Opts { level: Level::Balanced, caps: Caps { deflate: true }, budget: usize::MAX, now_ms: 0 }).unwrap().msg
}

fn sha_hex(b: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    iyakki_client_engine::sign::hex(&Sha256::digest(b))
}

#[test]
fn golden_frame_gives_the_same_pixels_in_both_decoders() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let mut theirs = pc::Canvas::default();
    let frame = golden_frame();
    pc::decode_frame(&frame, &mut theirs).unwrap();
    let want = serde_json::json!({ "w": theirs.w, "h": theirs.h, "seq": 1, "rgba_sha256": sha_hex(&theirs.rgba) });
    if std::env::var("IYAKKI_GOLDEN_WRITE").is_ok() {
        std::fs::write(dir.join("frame.bin"), &frame).unwrap();
        std::fs::write(dir.join("frame.json"), serde_json::to_string_pretty(&want).unwrap()).unwrap();
    }
    let stored = std::fs::read(dir.join("frame.bin")).expect("tests/golden/frame.bin");
    let meta: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("frame.json")).unwrap()).unwrap();
    let mut ours = picture::Canvas::default();
    picture::decode_frame(&stored, &mut ours).unwrap();
    let mut theirs = pc::Canvas::default();
    pc::decode_frame(&stored, &mut theirs).unwrap();
    assert_eq!(ours.rgba, theirs.rgba);
    assert_eq!(sha_hex(&ours.rgba), meta["rgba_sha256"].as_str().unwrap());
    assert_eq!((ours.w as u64, ours.h as u64), (meta["w"].as_u64().unwrap(), meta["h"].as_u64().unwrap()));
}
