//! A small CSV/TSV record reader (RFC 4180: quoted fields, doubled quotes, newlines inside quotes, CRLF, a UTF-8 BOM)
//! and writer, used by the built-in engine. It reports the byte offset of every record so a row can be fetched later
//! without reading the file again.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};

pub struct Records<R: BufRead + Seek> {
    r: R,
    delim: u8,
    /// Where the next record starts.
    pub pos: u64,
    line: Vec<u8>,
}

impl Records<BufReader<File>> {
    pub fn open(path: &str, delim: u8) -> std::io::Result<Self> {
        let mut r = BufReader::with_capacity(256 * 1024, File::open(path)?);
        // a UTF-8 byte order mark is not part of the first name
        let mut bom = [0u8; 3];
        let n = r.read(&mut bom)?;
        let pos = if n == 3 && bom == [0xEF, 0xBB, 0xBF] { 3 } else { 0 };
        r.seek(SeekFrom::Start(pos))?;
        Ok(Records { r, delim, pos, line: Vec::new() })
    }
}

impl<R: BufRead + Seek> Records<R> {
    pub fn seek(&mut self, offset: u64) -> std::io::Result<()> {
        if offset != self.pos {
            self.r.seek(SeekFrom::Start(offset))?;
            self.pos = offset;
        }
        Ok(())
    }

    /// The next non-empty record into `out` (cleared first); returns the offset where it started, or `None` at the end.
    pub fn next(&mut self, out: &mut Vec<String>) -> std::io::Result<Option<u64>> {
        loop {
            out.clear();
            let start = self.pos;
            let mut cur: Vec<u8> = Vec::new();
            let mut in_q = false;
            let mut at_start = true; // at the first byte of a field
            let mut content = false; // anything but a line break was seen
            let mut after_close = false; // just past the closing quote of a quoted field
            let mut any = false;
            loop {
                self.line.clear();
                let n = self.r.read_until(b'\n', &mut self.line)?;
                if n == 0 {
                    if !any {
                        return Ok(None);
                    }
                    break; // the file ends inside a record
                }
                self.pos += n as u64;
                any = true;
                let mut i = 0;
                let line = &self.line;
                while i < line.len() {
                    let b = line[i];
                    if !matches!(b, b'\r' | b'\n') || in_q {
                        content = true;
                    }
                    if in_q {
                        if b == b'"' {
                            if line.get(i + 1) == Some(&b'"') {
                                cur.push(b'"');
                                i += 1;
                            } else {
                                in_q = false;
                                after_close = true;
                            }
                        } else {
                            cur.push(b);
                        }
                    } else if b == self.delim {
                        out.push(String::from_utf8_lossy(&cur).into_owned());
                        cur.clear();
                        at_start = true;
                        after_close = false;
                    } else if b == b'"' && at_start {
                        in_q = true;
                        at_start = false;
                    } else if b == b'\n' || (b == b'\r' && i + 1 == line.len()) || (b == b'\r' && line.get(i + 1) == Some(&b'\n')) {
                        // the record ends (the line break is not data)
                    } else if after_close {
                        // junk after a closing quote: keep it
                        cur.push(b);
                    } else {
                        cur.push(b);
                        at_start = false;
                    }
                    i += 1;
                }
                if !in_q {
                    break;
                }
            }
            if !content {
                continue; // a blank line
            }
            out.push(String::from_utf8_lossy(&cur).into_owned());
            return Ok(Some(start));
        }
    }
}

/// One field of CSV output: quoted when it holds the delimiter, a quote or a line break.
pub fn field(s: &str, delim: char) -> String {
    if s.contains([delim, '"', '\n', '\r']) || s.starts_with(' ') || s.ends_with(' ') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

pub fn write_record<W: Write>(w: &mut W, cells: &[String], delim: char) -> std::io::Result<()> {
    let line: Vec<String> = cells.iter().map(|c| field(c, delim)).collect();
    w.write_all(line.join(&delim.to_string()).as_bytes())?;
    w.write_all(b"\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_all(bytes: &[u8]) -> Vec<(u64, Vec<String>)> {
        let p = std::env::temp_dir().join(format!("iyakki-csv-{}-{}", std::process::id(), bytes.len()));
        std::fs::write(&p, bytes).unwrap();
        let mut r = Records::open(p.to_str().unwrap(), b',').unwrap();
        let mut out = vec![];
        let mut f = vec![];
        while let Some(o) = r.next(&mut f).unwrap() {
            out.push((o, f.clone()));
        }
        let _ = std::fs::remove_file(p);
        out
    }

    #[test]
    fn quotes_newlines_crlf_and_bom() {
        let r = read_all(b"\xEF\xBB\xBFa,b\r\n1,\"x\ny\"\r\n\"say \"\"hi\"\"\",\n\n3,4");
        assert_eq!(r.len(), 4, "{r:?}");
        assert_eq!(r[0].1, vec!["a", "b"], "the BOM is dropped");
        assert_eq!(r[0].0, 3);
        assert_eq!(r[1].1, vec!["1", "x\ny"]);
        assert_eq!(r[2].1, vec!["say \"hi\"", ""]);
        assert_eq!(r[3].1, vec!["3", "4"], "a blank line is skipped and a last line needs no line break");
    }

    #[test]
    fn a_quote_inside_an_unquoted_field_is_just_text() {
        let r = read_all(b"5\" pipe,x\nnext,y\n");
        assert_eq!(r[0].1, vec!["5\" pipe", "x"]);
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn offsets_allow_seeking_back_to_a_record() {
        let p = std::env::temp_dir().join(format!("iyakki-csv-seek-{}", std::process::id()));
        std::fs::write(&p, "h\nr1\nr2\nr3\n").unwrap();
        let mut r = Records::open(p.to_str().unwrap(), b',').unwrap();
        let mut f = vec![];
        let mut offs = vec![];
        while let Some(o) = r.next(&mut f).unwrap() {
            offs.push(o);
        }
        r.seek(offs[2]).unwrap();
        r.next(&mut f).unwrap();
        assert_eq!(f, vec!["r2"]);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn output_quotes_only_when_needed() {
        assert_eq!(field("plain", ','), "plain");
        assert_eq!(field("a,b", ','), "\"a,b\"");
        assert_eq!(field("say \"x\"", ','), "\"say \"\"x\"\"\"");
        assert_eq!(field("a\tb", '\t'), "\"a\tb\"");
    }
}
