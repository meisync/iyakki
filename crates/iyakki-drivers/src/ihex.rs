//! Intel HEX to a Broadcom `.hcd` patch file.
//!
//! macOS keeps the Bluetooth patch RAM of several Broadcom controllers as Intel HEX text (`IOBluetooth.framework/Versions/A/Resources/
//! BCM4350-Updater.hex`, found in the macOS 11 recovery image; later macOS versions dropped them). Linux's `btbcm` wants a `.hcd`: a
//! plain sequence of HCI commands, each `opcode (2 bytes, little endian), parameter length (1 byte), parameters`. The usual
//! conversion (the community's `hex2hcd`, and what BrcmPatchRAM does at run time) is one vendor `Write_RAM` command (0xFC4C: a 4-byte
//! little-endian address, then up to 251 data bytes) for each stretch of data and a final `Launch_RAM` (0xFC4E) at address
//! 0xFFFFFFFF. That is all this does; nothing is guessed:
//!
//! * only data (00), end of file (01), extended segment (02), extended linear address (04) and start address (03, 05) records are
//!   understood; every record's checksum is verified; a line that does not start with `:` is skipped (Apple's files begin with a
//!   stray `$` line and have no end-of-file record);
//! * data that is not contiguous with the data before it starts a new `Write_RAM` run; nothing is merged across a gap;
//! * the input is bounded ([`MAX_HEX_BYTES`]) and every length is checked: no panics, a plain error for anything else.
//!
//! Verified only on bytes: the output was built from the real `BCM4350-Updater.hex` of macOS 11.6 (see `docs/MAC-WIFI-BLUETOOTH-SETUP.md`
//! for its hashes). Whether the controller of a Mac accepts it is for a real Mac to say.

/// Largest hex text read (the real files are 20 to 200 KB).
pub const MAX_HEX_BYTES: usize = 4 << 20;
/// Data bytes in one `Write_RAM` command (the command's parameter length is at most 255: 4 address bytes plus these).
pub const CHUNK: usize = 248;

pub const OP_WRITE_RAM: u16 = 0xFC4C;
pub const OP_LAUNCH_RAM: u16 = 0xFC4E;

/// The data of a hex file as `(address, bytes)` runs, in file order, contiguous data joined.
pub fn parse(text: &[u8]) -> Result<Vec<(u32, Vec<u8>)>, String> {
    if text.len() > MAX_HEX_BYTES {
        return Err(format!("the hex file is {} bytes, more than the {MAX_HEX_BYTES} allowed", text.len()));
    }
    let mut runs: Vec<(u32, Vec<u8>)> = Vec::new();
    let mut base: u32 = 0;
    let mut records = 0usize;
    for (n, line) in String::from_utf8_lossy(text).lines().enumerate() {
        let line = line.trim();
        let Some(h) = line.strip_prefix(':') else { continue };
        let raw = unhex(h).ok_or_else(|| format!("hex line {}: not hexadecimal", n + 1))?;
        if raw.len() < 5 {
            return Err(format!("hex line {}: too short", n + 1));
        }
        let len = raw[0] as usize;
        if raw.len() != len + 5 {
            return Err(format!("hex line {}: the length byte says {len} data bytes, the line has {}", n + 1, raw.len().saturating_sub(5)));
        }
        if raw.iter().fold(0u8, |a, b| a.wrapping_add(*b)) != 0 {
            return Err(format!("hex line {}: checksum is wrong", n + 1));
        }
        let off = u16::from_be_bytes([raw[1], raw[2]]) as u32;
        let data = &raw[4..4 + len];
        records += 1;
        match raw[3] {
            0 => {
                let addr = base.wrapping_add(off);
                match runs.last_mut() {
                    Some((a, v)) if (*a as u64 + v.len() as u64) == addr as u64 => v.extend_from_slice(data),
                    _ => runs.push((addr, data.to_vec())),
                }
            }
            1 => break,
            2 if len == 2 => base = (u16::from_be_bytes([data[0], data[1]]) as u32) << 4,
            4 if len == 2 => base = (u16::from_be_bytes([data[0], data[1]]) as u32) << 16,
            3 | 5 if len == 4 => {}
            t => return Err(format!("hex line {}: record type {t:02x} with {len} data bytes is not understood", n + 1)),
        }
    }
    if runs.is_empty() {
        return Err(format!("the hex file has no data ({records} records)"));
    }
    Ok(runs)
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if b.len() % 2 != 0 {
        return None;
    }
    b.chunks(2).map(|p| Some((nib(p[0])? << 4) | nib(p[1])?)).collect()
}

fn nib(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

fn command(out: &mut Vec<u8>, opcode: u16, params: &[u8]) {
    out.extend_from_slice(&opcode.to_le_bytes());
    out.push(params.len() as u8);
    out.extend_from_slice(params);
}

/// Intel HEX text to `.hcd` bytes: `Write_RAM` for every [`CHUNK`] of every run, then `Launch_RAM` at 0xFFFFFFFF.
pub fn to_hcd(text: &[u8]) -> Result<Vec<u8>, String> {
    let runs = parse(text)?;
    let mut out = Vec::new();
    for (addr, data) in runs {
        for (i, c) in data.chunks(CHUNK).enumerate() {
            let a = (addr as u64 + (i * CHUNK) as u64) as u32;
            let mut p = a.to_le_bytes().to_vec();
            p.extend_from_slice(c);
            command(&mut out, OP_WRITE_RAM, &p);
        }
    }
    command(&mut out, OP_LAUNCH_RAM, &0xFFFF_FFFFu32.to_le_bytes());
    Ok(out)
}

/// Is this a well-formed `.hcd`: commands that add up exactly, all `Write_RAM` except one last `Launch_RAM`? Returns the number of data bytes.
pub fn check_hcd(b: &[u8]) -> Result<usize, String> {
    let (mut i, mut data, mut launched) = (0usize, 0usize, false);
    while i < b.len() {
        if launched {
            return Err("commands after the Launch_RAM".into());
        }
        let hdr = b.get(i..i + 3).ok_or("a command is cut off")?;
        let (op, n) = (u16::from_le_bytes([hdr[0], hdr[1]]), hdr[2] as usize);
        let p = b.get(i + 3..i + 3 + n).ok_or("a command is cut off")?;
        match op {
            OP_WRITE_RAM if n > 4 => data += n - 4,
            OP_LAUNCH_RAM if n == 4 => launched = true,
            _ => return Err(format!("unexpected command {op:04x} with {} parameter bytes", p.len())),
        }
        i += 3 + n;
    }
    if !launched {
        return Err("no Launch_RAM at the end".into());
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(t: u8, addr: u16, data: &[u8]) -> String {
        let mut b = vec![data.len() as u8, (addr >> 8) as u8, addr as u8, t];
        b.extend_from_slice(data);
        let sum = b.iter().fold(0u8, |a, x| a.wrapping_add(*x));
        b.push(0u8.wrapping_sub(sum));
        format!(":{}", b.iter().map(|x| format!("{x:02X}")).collect::<String>())
    }

    #[test]
    fn a_hex_file_becomes_write_ram_commands_and_a_launch() {
        // Apple's shape: a stray first line, an extended linear address, data, no end-of-file record
        let t = format!("$\n{}\n{}\n{}\n{}\n", rec(4, 0, &[0xFF, 0x00]), rec(0, 0x4000, &[1, 2, 3, 4]), rec(0, 0x4004, &[5, 6]), rec(0, 0x4100, &[9]));
        let hcd = to_hcd(t.as_bytes()).unwrap();
        let want: Vec<u8> = [
            &[0x4C, 0xFC, 10, 0x00, 0x40, 0x00, 0xFF, 1, 2, 3, 4, 5, 6][..], // joined with the next record: 6 data bytes, length 10
            &[0x4C, 0xFC, 5, 0x00, 0x41, 0x00, 0xFF, 9][..],                  // the gap starts a new run
            &[0x4E, 0xFC, 4, 0xFF, 0xFF, 0xFF, 0xFF][..],
        ]
        .concat();
        assert_eq!(hcd, want);
        assert_eq!(check_hcd(&hcd), Ok(7));
    }

    #[test]
    fn long_runs_are_cut_into_commands_that_fit_and_addresses_advance() {
        let data: Vec<u8> = (0..600u32).map(|i| i as u8).collect();
        let mut t = String::new();
        for (i, c) in data.chunks(32).enumerate() {
            t.push_str(&rec(0, (0x1000 + i * 32) as u16, c));
            t.push('\n');
        }
        let hcd = to_hcd(t.as_bytes()).unwrap();
        // 600 = 248 + 248 + 104
        assert_eq!(&hcd[..3], &[0x4C, 0xFC, 252]);
        let second = 3 + 252;
        assert_eq!(&hcd[second..second + 7], &[0x4C, 0xFC, 252, 0xF8, 0x10, 0, 0], "second address = 0x1000 + 248");
        assert_eq!(check_hcd(&hcd), Ok(600));
    }

    #[test]
    fn a_bad_checksum_garbage_and_too_much_are_errors_not_panics() {
        let mut bad = rec(0, 0, &[1, 2, 3]);
        bad.replace_range(bad.len() - 2.., "00");
        assert!(to_hcd(bad.as_bytes()).unwrap_err().contains("checksum"));
        assert!(to_hcd(b":zz\n").unwrap_err().contains("not hexadecimal"));
        assert!(to_hcd(b":0100").unwrap_err().contains("too short") || to_hcd(b":0100").is_err());
        assert!(to_hcd(b":0300000102").is_err());
        assert!(to_hcd(b"$\nnothing here\n").unwrap_err().contains("no data"));
        assert!(to_hcd(&vec![b'0'; MAX_HEX_BYTES + 1]).unwrap_err().contains("more than"));
        assert!(to_hcd(rec(9, 0, &[1]).as_bytes()).unwrap_err().contains("not understood"));
        for junk in [&b""[..], b":", b":00", b"\xff\xfe:\n", b":FFFFFFFF"] {
            let _ = to_hcd(junk); // must not panic
        }
    }

    #[test]
    fn end_of_file_stops_reading_and_segment_addresses_work() {
        let t = format!("{}\n{}\n{}\n{}\n", rec(2, 0, &[0x10, 0x00]), rec(0, 0x0010, &[7]), rec(1, 0, &[]), rec(0, 0, &[8]));
        let runs = parse(t.as_bytes()).unwrap();
        assert_eq!(runs, vec![(0x10010, vec![7])]);
    }

    #[test]
    fn check_hcd_refuses_cut_off_and_foreign_commands() {
        assert!(check_hcd(&[0x4C, 0xFC, 9, 0]).is_err());
        assert!(check_hcd(&[0x03, 0x0C, 0]).is_err());
        assert!(check_hcd(&[0x4E, 0xFC, 4, 0xFF, 0xFF, 0xFF, 0xFF, 0x4C, 0xFC, 5, 0, 0, 0, 0, 1]).is_err());
        assert!(check_hcd(&[0x4C, 0xFC, 5, 0, 0, 0, 0, 1]).is_err(), "no launch");
    }

    /// Real bytes, not in the repository: `IYAKKI_REAL_HEX=/path/BCM4350-Updater.hex cargo test -p iyakki-drivers --lib real_hex -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_hex_converts_and_prints_its_size_and_hash() {
        use sha2::{Digest, Sha256};
        let Ok(p) = std::env::var("IYAKKI_REAL_HEX") else { return };
        let b = std::fs::read(p).unwrap();
        let h = to_hcd(&b).unwrap();
        let data = check_hcd(&h).unwrap();
        let sum: String = Sha256::digest(&h).iter().map(|x| format!("{x:02x}")).collect();
        println!("hcd {} bytes, {data} data bytes, sha256 {sum}", h.len());
    }
}
