//! Varint writer and bounds-checked reader.
use alloc::{string::String, vec::Vec};

/// Why a buffer was refused. Plain, stable names: hosts show `Display` text to the person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    BadMagic,
    /// The buffer is for another ABI version (the number found).
    BadVersion(u16),
    BadTag(u8),
    TooManyItems,
    TooLarge,
    /// A coordinate, size or text size is outside the allowed range.
    OutOfRange,
    BadUtf8,
    /// Clip pushes and pops do not match.
    UnbalancedClip,
    TrailingBytes,
}

impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let s = match self {
            DecodeError::Truncated => "the data ends too early",
            DecodeError::BadMagic => "this is not tile data (wrong marker)",
            DecodeError::BadVersion(_) => "the data is for another tile ABI version",
            DecodeError::BadTag(_) => "unknown item kind",
            DecodeError::TooManyItems => "too many items",
            DecodeError::TooLarge => "the data is too large",
            DecodeError::OutOfRange => "a number is out of the allowed range",
            DecodeError::BadUtf8 => "text is not valid UTF-8",
            DecodeError::UnbalancedClip => "clip push and pop do not match",
            DecodeError::TrailingBytes => "extra bytes after the end",
        };
        f.write_str(s)
    }
}

#[derive(Default)]
pub(crate) struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    pub fn u16le(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn var(&mut self, mut v: u32) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.buf.push(b);
                return;
            }
            self.buf.push(b | 0x80);
        }
    }
    pub fn zz(&mut self, v: i32) {
        self.var(((v << 1) ^ (v >> 31)) as u32);
    }
    pub fn bytes(&mut self, b: &[u8]) {
        self.var(b.len() as u32);
        self.buf.extend_from_slice(b);
    }
}

pub(crate) struct Reader<'a> {
    d: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(d: &'a [u8]) -> Self {
        Reader { d, pos: 0 }
    }
    pub fn done(&self) -> bool {
        self.pos == self.d.len()
    }
    pub fn u8(&mut self) -> Result<u8, DecodeError> {
        let b = *self.d.get(self.pos).ok_or(DecodeError::Truncated)?;
        self.pos += 1;
        Ok(b)
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.pos.checked_add(n).ok_or(DecodeError::Truncated)?;
        let s = self.d.get(self.pos..end).ok_or(DecodeError::Truncated)?;
        self.pos = end;
        Ok(s)
    }
    pub fn u16le(&mut self) -> Result<u16, DecodeError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    pub fn var(&mut self) -> Result<u32, DecodeError> {
        let mut v: u32 = 0;
        for i in 0..5 {
            let b = self.u8()?;
            let part = (b & 0x7f) as u32;
            if i == 4 && part > 0x0f {
                return Err(DecodeError::OutOfRange);
            }
            v |= part << (7 * i);
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(DecodeError::OutOfRange)
    }
    pub fn zz(&mut self) -> Result<i32, DecodeError> {
        let v = self.var()?;
        Ok(((v >> 1) as i32) ^ -((v & 1) as i32))
    }
    /// A coordinate or size: zigzag, within the ABI range.
    pub fn coord(&mut self) -> Result<i32, DecodeError> {
        let v = self.zz()?;
        if v.unsigned_abs() > crate::limits::MAX_COORD as u32 {
            return Err(DecodeError::OutOfRange);
        }
        Ok(v)
    }
    pub fn string(&mut self) -> Result<String, DecodeError> {
        let n = self.var()? as usize;
        if n > crate::limits::MAX_TEXT {
            return Err(DecodeError::TooLarge);
        }
        let b = self.take(n)?;
        core::str::from_utf8(b).map(String::from).map_err(|_| DecodeError::BadUtf8)
    }
}
