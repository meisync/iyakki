use super::*;

#[test]
fn a_chunk_is_20_ms_at_48_khz() {
    assert_eq!(CHUNK_FRAMES, 960);
    assert_eq!(MAX_QUEUE_CHUNKS, 5);
}

#[test]
fn audio_round_trips_and_refuses_ragged_payloads() {
    let pcm: Vec<i16> = (0..1920).map(|i| (i as i16).wrapping_mul(7)).collect();
    let p = encode_audio(41, &pcm);
    assert_eq!(p.len(), 4 + 3840);
    assert_eq!(decode_audio(&p, 2), Some((41, pcm.clone())));
    assert_eq!(decode_audio(&p[..p.len() - 1], 2), None, "half a sample");
    assert_eq!(decode_audio(&p[..p.len() - 2], 2), None, "half a frame");
    assert_eq!(decode_audio(&p[..3], 2), None);
    assert_eq!(decode_audio(&p, 0), None);
    let huge = encode_audio(1, &vec![0i16; 48_000 * 2 + 2]);
    assert_eq!(decode_audio(&huge, 2), None, "more than a second is not a chunk");
}

#[test]
fn camera_frames_round_trip_and_are_bounded() {
    let j = [0xFF, 0xD8, 1, 2, 3, 0xFF, 0xD9];
    let p = encode_camera("usb:video0", 9, &j).unwrap();
    assert_eq!(decode_camera(&p), Some(("usb:video0".to_string(), 9, &j[..])));
    assert!(encode_camera(&"u".repeat(MAX_UNIT + 1), 1, &j).is_none());
    assert!(encode_camera("u", 1, &vec![0; MAX_CAMERA_FRAME + 1]).is_none());
    assert_eq!(decode_camera(&[5, b'a']), None);
    assert_eq!(decode_camera(&[]), None);
    assert!(looks_like_jpeg(&j));
    assert!(!looks_like_jpeg(&[0xFF, 0xD8, 0xFF]));
    assert!(!looks_like_jpeg(b"<html>"));
}

#[test]
fn the_side_defaults_to_the_client_and_both_is_only_by_name() {
    assert_eq!(Side::parse(None), Some(Side::Client));
    assert_eq!(Side::parse(Some("")), Some(Side::Client));
    assert_eq!(Side::parse(Some("client")), Some(Side::Client));
    assert_eq!(Side::parse(Some("both")), Some(Side::Both));
    assert_eq!(Side::parse(Some("machine")), None, "the machine alone needs no route");
    assert_eq!(Side::parse(Some("everywhere")), None);
}

#[test]
fn clipboard_text_is_capped_and_cleaned() {
    assert_eq!(clean_clipboard("hello\nworld\u{0}!"), Ok("hello\nworld!".to_string()));
    assert_eq!(clean_clipboard(""), Err(ClipError::Empty));
    assert_eq!(clean_clipboard("\u{0}\u{0}"), Err(ClipError::Empty));
    assert!(clean_clipboard(&"a".repeat(MAX_CLIPBOARD)).is_ok());
    assert_eq!(clean_clipboard(&"a".repeat(MAX_CLIPBOARD + 1)), Err(ClipError::TooBig));
    // the cap is in bytes, not characters
    assert_eq!(clean_clipboard(&"é".repeat(MAX_CLIPBOARD / 2 + 1)), Err(ClipError::TooBig));
}

#[test]
fn virtual_source_names_follow_the_device() {
    assert_eq!(source_name("Maya's laptop", SourceKind::Camera), "Maya's laptop camera");
    assert_eq!(source_name("Maya's laptop", SourceKind::Microphone), "Maya's laptop microphone");
    assert_eq!(source_name("  ", SourceKind::Camera), "client camera");
    assert_eq!(source_name("a\nb", SourceKind::Camera), "ab camera");
    assert_eq!(SourceKind::parse("camera"), Some(SourceKind::Camera));
    assert_eq!(SourceKind::parse("screen"), None);
    assert_eq!(SourceKind::Camera.capability(), "client.camera.offer");
}

#[test]
fn the_chunker_cuts_whole_20_ms_chunks_and_keeps_the_rest() {
    let mut c = Chunker::new(2);
    assert!(c.push(&vec![1; 1000]).is_empty());
    let out = c.push(&vec![2; 3000]);
    assert_eq!(out.len(), 2);
    assert!(out.iter().all(|x| x.len() == 1920));
    assert_eq!(out[0][0], 1);
    assert_eq!(out[1][1919], 2);
    // 4000 pushed, 3840 cut, 160 carried
    assert!(c.push(&vec![3; 1760]).len() == 1);
    c.push(&[0; 100]);
    c.clear();
    assert!(c.push(&vec![0; 1919]).is_empty(), "a stop leaves no half chunk behind");
}

#[test]
fn the_jitter_queue_never_holds_more_than_100_ms_and_drops_the_oldest() {
    let mut q = JitterQueue::bounded();
    for i in 0..12u32 {
        q.push(i);
    }
    assert_eq!(q.len(), 5);
    assert_eq!(q.dropped(), 7);
    assert_eq!(q.pop(), Some(7), "the oldest were thrown away, the newest 100 ms are kept");
    assert_eq!(q.clear(), 4);
    assert!(q.is_empty());
    assert_eq!(q.pop(), None);
}
