//! Sound conversion helpers the speech code needs (moved from the shell-side `audioout`).

/// A WAV whose samples are not 16-bit PCM (32- or 64-bit float, 24- or 32-bit integer: Lemonade's Kokoro answers 32-bit float
/// mono at 24 kHz), as a 16-bit PCM WAV with the same rate and channels. `None` when it already is 16-bit PCM or is not a WAV
/// this can read. Why: the output goes through ALSA's `plug` -> `softvol` chain, whose rate converter cannot take float
/// samples ("Unable to install hw params ... RATE: NONE"), so a float WAV was silent on cards that need rate conversion.
pub fn to_pcm16_wav(wav: &[u8]) -> Option<Vec<u8>> {
    if wav.len() < 44 || &wav[..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return None;
    }
    let (mut pos, mut fmt, mut data): (usize, Option<(u16, u16, u32, u16)>, Option<(usize, usize)>) = (12, None, None);
    while pos + 8 <= wav.len() {
        let id = &wav[pos..pos + 4];
        let len = u32::from_le_bytes(wav[pos + 4..pos + 8].try_into().ok()?) as usize;
        let body = pos + 8;
        if id == b"fmt " && wav.len() >= body + 16 {
            fmt = Some((u16::from_le_bytes([wav[body], wav[body + 1]]), u16::from_le_bytes([wav[body + 2], wav[body + 3]]), u32::from_le_bytes(wav[body + 4..body + 8].try_into().ok()?), u16::from_le_bytes([wav[body + 14], wav[body + 15]])));
            // WAVE_FORMAT_EXTENSIBLE: the real format tag is the first two bytes of the sub-format GUID
            if fmt.map(|f| f.0) == Some(0xFFFE) && wav.len() >= body + 26 && len >= 26 {
                fmt = fmt.map(|f| (u16::from_le_bytes([wav[body + 24], wav[body + 25]]), f.1, f.2, f.3));
            }
        } else if id == b"data" {
            // a stream written as it is made may say 0 or 0xFFFFFFFF: the data runs to the end
            let end = if len == 0 || len == u32::MAX as usize { wav.len() } else { body.saturating_add(len).min(wav.len()) };
            data = Some((body, end));
            break;
        }
        pos = body.saturating_add(len).saturating_add(len & 1);
    }
    let (tag, channels, rate, bits) = fmt?;
    let (start, end) = data?;
    if channels == 0 || rate == 0 || end < start {
        return None;
    }
    let raw = &wav[start..end];
    let pcm: Vec<i16> = match (tag, bits) {
        (3, 32) => raw.chunks_exact(4).map(|c| (f32::from_le_bytes([c[0], c[1], c[2], c[3]]).clamp(-1.0, 1.0) * 32767.0) as i16).collect(),
        (3, 64) => raw.chunks_exact(8).map(|c| (f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]).clamp(-1.0, 1.0) * 32767.0) as i16).collect(),
        (1, 24) => raw.chunks_exact(3).map(|c| i16::from_le_bytes([c[1], c[2]])).collect(),
        (1, 32) => raw.chunks_exact(4).map(|c| i16::from_le_bytes([c[2], c[3]])).collect(),
        (1, 8) => raw.iter().map(|&b| ((b as i16) - 128) << 8).collect(),
        _ => return None, // 16-bit PCM (nothing to do) or a format this does not know
    };
    let frames = pcm.len() / channels as usize * channels as usize;
    let data_len = (frames * 2) as u32;
    let mut o = Vec::with_capacity(44 + frames * 2);
    o.extend_from_slice(b"RIFF");
    o.extend((36 + data_len).to_le_bytes());
    o.extend_from_slice(b"WAVEfmt ");
    o.extend(16u32.to_le_bytes());
    o.extend(1u16.to_le_bytes());
    o.extend(channels.to_le_bytes());
    o.extend(rate.to_le_bytes());
    o.extend((rate * channels as u32 * 2).to_le_bytes());
    o.extend((channels * 2).to_le_bytes());
    o.extend(16u16.to_le_bytes());
    o.extend_from_slice(b"data");
    o.extend(data_len.to_le_bytes());
    for s in &pcm[..frames] {
        o.extend(s.to_le_bytes());
    }
    Some(o)
}

/// Does this argument list already name an ALSA device (`-D x`, `-Dx`, `-qD x`, `--device=x`)?
pub fn names_device(args: &[String]) -> bool {
    args.iter().any(|a| a == "--device" || a.starts_with("--device=") || (a.starts_with('-') && !a.starts_with("--") && a[1..].contains('D')))
}

/// Join an argument list back into a command line that `crate::voice::split_command` reads the same way.
pub fn join_command(args: &[String]) -> String {
    args.iter()
        .map(|a| if !a.is_empty() && !a.contains(|c: char| c.is_whitespace() || c == '\'' || c == '"' || c == '\\') { a.clone() } else if !a.contains('\'') { format!("'{a}'") } else { format!("\"{}\"", a.replace('\\', "\\\\").replace('"', "\\\"")) })
        .collect::<Vec<_>>()
        .join(" ")
}
