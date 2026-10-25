use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// What `aplay -D hw:0,0 --dump-hw-params` printed on the MacBookPro14,1 (CS8409, Apple path).
const MACBOOK: &str = "HW Params of device \"hw:0,0\":\n--------------------\nACCESS:  MMAP_INTERLEAVED RW_INTERLEAVED\nFORMAT:  S24_LE S32_LE\nSUBFORMAT:  STD\nSAMPLE_BITS: 32\nFRAME_BITS: [64 128]\nCHANNELS: [2 4]\nRATE: 44100\nPERIOD_TIME: (181 5944308)\nPERIOD_SIZE: [8 262144]\nPERIOD_BYTES: [64 2097152]\nPERIODS: [2 32]\nBUFFER_TIME: (362 11888617)\nBUFFER_SIZE: [16 524288]\nBUFFER_BYTES: [128 4194304]\nTICK_TIME: ALL\n--------------------\n";

fn r(lo: u32, hi: u32) -> Range {
    Range { lo, hi }
}

#[test]
fn the_macbook_report_is_read_and_gives_a_44100_hz_32_bit_dmix_with_two_channels() {
    let c = parse(MACBOOK);
    assert_eq!(c.formats, ["S24_LE", "S32_LE"]);
    assert_eq!(c.channels, [r(2, 4)]);
    assert_eq!(c.rates, [r(44100, 44100)]);
    assert_eq!(c.period_size, [r(8, 262144)]);
    assert_eq!(c.fixed_rate(), Some(44100));
    let m = c.mix().unwrap();
    // 16-bit is refused, so 32-bit; 2 channels are possible (the owner heard four channels, a route plugin handles that case
    // for devices that need them); the only rate is kept
    assert_eq!((m.format, m.channels, m.rate, m.sized, m.probed), ("S32_LE", 2, 44100, true, true));
}

#[test]
fn an_ordinary_card_gives_exactly_the_long_standing_stereo_mix() {
    let c = parse("FORMAT: S16_LE S24_LE\nCHANNELS: [1 2]\nRATE: [8000 192000]\nPERIOD_SIZE: [16 65536]\nBUFFER_SIZE: [32 131072]\n");
    assert_eq!(c.mix(), Some(Mix::STEREO));
    assert_eq!(parse("FORMAT: S16_LE S24_LE\nCHANNELS: 2\nRATE: [8000 192000]\n").mix().map(|m| m.sized), Some(false), "no period line: the device's own sizes");
}

#[test]
fn only_four_channels_use_four_and_mono_only_uses_one_and_the_rate_is_the_nearest() {
    let m = parse("FORMAT: S32_LE\nCHANNELS: 4\nRATE: [8000 44100]\n").mix().unwrap();
    assert_eq!((m.format, m.channels, m.rate), ("S32_LE", 4, 44100), "48 kHz is out of range: the nearest rate");
    assert!(m.fans_out());
    assert_eq!(parse("FORMAT: S16_LE\nCHANNELS: 1\nRATE: 48000\n").mix().map(|m| m.channels), Some(1));
    // formats: 24-bit in 32 is preferred over the packed one, the packed one is the last resort
    assert_eq!(parse("FORMAT: S24_LE S24_3LE\nCHANNELS: 2\nRATE: 48000\n").mix().map(|m| m.format), Some("S24_LE"));
    assert_eq!(parse("FORMAT: S24_3LE\nCHANNELS: 2\nRATE: 48000\n").mix().map(|m| m.format), Some("S24_3LE"));
    // 48000 inside a list of two rates
    assert_eq!(parse("FORMAT: S16_LE\nCHANNELS: 2\nRATE: 44100 48000\n").mix().map(|m| m.rate), Some(48000));
}

#[test]
fn garbled_or_hostile_output_never_panics_and_gives_no_mix() {
    for t in ["", "\0\0\0", "FORMAT:", "FORMAT: ???\nCHANNELS: [\nRATE: ]\n", "CHANNELS: [4 2]\nRATE: (5 5)\nFORMAT: S16_LE", "RATE: [99999999999999 1]\nCHANNELS: -3\nFORMAT: S16_LE", "::::\n:\nRATE:::[1 2 3 4 5]", &"FORMAT: S16_LE ".repeat(100_000), &"RATE: 1 ".repeat(100_000), "CHANNELS: [2 99]\nRATE: 48000\nFORMAT: S16_LE"] {
        let c = parse(t);
        let _ = c.mix(); // must not panic
    }
    assert_eq!(parse("CHANNELS: [4 2]\nRATE: 48000\nFORMAT: S16_LE").mix(), None, "an empty range is no range");
    assert_eq!(parse("FORMAT: S16_LE\nCHANNELS: 2\n").mix(), None, "no rate line");
    assert_eq!(parse("FORMAT: S16_LE\nCHANNELS: 64\nRATE: 48000").mix(), None, "a channel count nobody plays");
    // an open lower bound excludes the number itself
    assert_eq!(parse("RATE: (8000 192000]").rates, [r(8001, 192000)]);
}

#[test]
fn the_answer_is_asked_once_per_device_and_a_failure_is_not_kept_as_an_answer() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c2 = calls.clone();
    let p = Prober::new(move |_, _| {
        if c2.fetch_add(1, Ordering::SeqCst) == 0 { Err("Device or resource busy".into()) } else { Ok(MACBOOK.to_string()) }
    })
    .with_retry(Duration::ZERO);
    assert_eq!(p.mix(0, 0), None, "busy: the caller keeps its default");
    assert_eq!(p.mix(0, 0).map(|m| m.rate), Some(44100));
    assert_eq!(p.mix(0, 0).map(|m| m.rate), Some(44100));
    assert_eq!(calls.load(Ordering::SeqCst), 2, "asked twice (the first failed), then remembered");
    p.forget(0);
    p.mix(0, 0);
    assert_eq!(calls.load(Ordering::SeqCst), 3, "a card that came back is asked again");
    // a failed look waits before asking again
    let calls = Arc::new(AtomicUsize::new(0));
    let c2 = calls.clone();
    let p = Prober::new(move |_, _| {
        c2.fetch_add(1, Ordering::SeqCst);
        Err("no".into())
    });
    for _ in 0..5 {
        assert!(p.mix(1, 0).is_none());
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // text that is not a report counts as a failure too
    assert!(Prober::new(|_, _| Ok("hello".into())).mix(0, 0).is_none());
}
