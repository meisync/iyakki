//! The "min reader version" header rule for binary formats (F-CORE-23, finding F-8 of CORE-COMPATIBILITY-DESIGN.md section 7).
//! A file says which reader version it needs at least. A reader older than that refuses with one plain sentence and touches nothing.
//! Pure logic: the formats call it after they have read their own header.

/// The sentence a person sees when the file needs a newer IYAKKI.
pub const UPDATE_SENTENCE: &str = "Update IYAKKI to open this.";

/// Why a header was not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderRefusal {
    /// The first bytes are not the format's magic.
    WrongMagic,
    /// The header is cut short.
    Short,
    /// The file needs a reader of at least `needs`; this reader is `have`.
    NeedsNewerReader { needs: u32, have: u32 },
}

impl HeaderRefusal {
    /// One plain sentence for the person.
    pub fn sentence(&self) -> String {
        match self {
            HeaderRefusal::WrongMagic => "This is not a file of this kind.".to_string(),
            HeaderRefusal::Short => "This file is cut short.".to_string(),
            HeaderRefusal::NeedsNewerReader { .. } => UPDATE_SENTENCE.to_string(),
        }
    }
}

/// What a good header gives back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Version the writer wrote.
    pub format_version: u32,
    /// Lowest reader version that may open it.
    pub min_reader: u32,
    /// Offset of the first byte after the header.
    pub body_at: usize,
}

/// Header layout: magic, format version (u32 LE), min reader version (u32 LE).
pub fn read_header(bytes: &[u8], magic: &[u8], reader_version: u32) -> Result<Header, HeaderRefusal> {
    if bytes.len() < magic.len() || &bytes[..magic.len()] != magic {
        return Err(if bytes.len() < magic.len() && magic.starts_with(bytes) && !bytes.is_empty() {
            HeaderRefusal::Short
        } else {
            HeaderRefusal::WrongMagic
        });
    }
    let at = magic.len();
    if bytes.len() < at + 8 {
        return Err(HeaderRefusal::Short);
    }
    let rd = |o: usize| u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
    let format_version = rd(at);
    let min_reader = rd(at + 4);
    if min_reader > reader_version {
        return Err(HeaderRefusal::NeedsNewerReader { needs: min_reader, have: reader_version });
    }
    Ok(Header { format_version, min_reader, body_at: at + 8 })
}

/// Write a header (for new formats and tests).
pub fn write_header(magic: &[u8], format_version: u32, min_reader: u32) -> Vec<u8> {
    let mut v = magic.to_vec();
    v.extend_from_slice(&format_version.to_le_bytes());
    v.extend_from_slice(&min_reader.to_le_bytes());
    v
}

/// Length-prefixed sections after the header: (tag u32 LE, length u32 LE, bytes). A reader keeps the tags it knows and skips the others.
pub fn sections<'a>(body: &'a [u8], known: &[u32]) -> Result<Vec<(u32, &'a [u8])>, HeaderRefusal> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < body.len() {
        if body.len() - at < 8 {
            return Err(HeaderRefusal::Short);
        }
        let tag = u32::from_le_bytes([body[at], body[at + 1], body[at + 2], body[at + 3]]);
        let len = u32::from_le_bytes([body[at + 4], body[at + 5], body[at + 6], body[at + 7]]) as usize;
        at += 8;
        if body.len() - at < len {
            return Err(HeaderRefusal::Short);
        }
        if known.contains(&tag) {
            out.push((tag, &body[at..at + len]));
        }
        at += len;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    const M: &[u8] = b"IYTEST01";

    #[test]
    fn an_old_enough_reader_opens_and_a_newer_need_is_one_plain_sentence() {
        let h = write_header(M, 3, 2);
        let got = read_header(&h, M, 2).unwrap();
        assert_eq!((got.format_version, got.min_reader, got.body_at), (3, 2, h.len()));
        assert!(read_header(&h, M, 5).is_ok());
        let e = read_header(&h, M, 1).unwrap_err();
        assert_eq!(e, HeaderRefusal::NeedsNewerReader { needs: 2, have: 1 });
        assert_eq!(e.sentence(), UPDATE_SENTENCE);
    }

    #[test]
    fn another_magic_and_a_cut_header_are_refused_apart() {
        assert_eq!(read_header(b"OTHERMAG\0\0\0\0\0\0\0\0", M, 9), Err(HeaderRefusal::WrongMagic));
        assert_eq!(read_header(b"", M, 9), Err(HeaderRefusal::WrongMagic));
        assert_eq!(read_header(b"IYTE", M, 9), Err(HeaderRefusal::Short));
        let mut h = write_header(M, 1, 1);
        h.truncate(h.len() - 2);
        assert_eq!(read_header(&h, M, 9), Err(HeaderRefusal::Short));
    }

    #[test]
    fn unknown_sections_are_skipped_and_a_cut_one_is_refused() {
        let mut b = Vec::new();
        for (tag, data) in [(1u32, &b"aa"[..]), (99, b"future"), (2, b"b")] {
            b.extend_from_slice(&tag.to_le_bytes());
            b.extend_from_slice(&(data.len() as u32).to_le_bytes());
            b.extend_from_slice(data);
        }
        let got = sections(&b, &[1, 2]).unwrap();
        assert_eq!(got, vec![(1, &b"aa"[..]), (2, &b"b"[..])]);
        b.truncate(b.len() - 1);
        assert_eq!(sections(&b, &[1, 2]), Err(HeaderRefusal::Short));
    }
}
