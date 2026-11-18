//! `iyakki-audiodec`: reads an Ogg Vorbis stream on the standard input and writes `IYPC`, the sample rate (u32, little-endian), the
//! channel count (u16, little-endian) and then interleaved 16-bit little-endian samples on the standard output. It takes no
//! arguments and touches no file and no network, so it can be started with nothing but two pipes. When it cannot decode the stream
//! it says why on the standard error and ends with status 2.

use std::io::{Read, Write};

fn main() {
    let mut input = std::io::stdin().lock();
    let mut out = std::io::stdout().lock();
    let mut dec = iyakki_audiodec::StreamDecoder::new();
    let mut wrote_header = false;
    let mut buf = vec![0u8; 16384];
    loop {
        let n = match input.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        let pcm = match dec.push(&buf[..n]) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(2);
            }
        };
        if !wrote_header && dec.ready() {
            wrote_header = true;
            if out.write_all(&iyakki_audiodec::header(dec.rate, dec.channels as u16)).is_err() {
                return;
            }
        }
        if !pcm.is_empty() {
            let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
            if out.write_all(&bytes).and_then(|_| out.flush()).is_err() {
                return;
            }
        }
    }
    if !wrote_header {
        eprintln!("no Ogg Vorbis audio was found in the stream");
        std::process::exit(2);
    }
}
