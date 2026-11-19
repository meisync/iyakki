//! The Ogg page layer: pages are found by their capture pattern, checked by their CRC, and their packets put together
//! (a packet may continue over many pages).

/// One checked page and the packets that ended on it.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub serial: u32,
    pub seq: u32,
    pub bos: bool,
    pub eos: bool,
    /// the granule position, -1 when the page ends no packet
    pub granule: i64,
    pub packets: Vec<Vec<u8>>,
}

/// The Ogg CRC-32 (polynomial 0x04c11db7, no reflection, no final xor).
pub fn crc(data: &[u8]) -> u32 {
    static T: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = T.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut r = (i as u32) << 24;
            for _ in 0..8 {
                r = if r & 0x8000_0000 != 0 { (r << 1) ^ 0x04c1_1db7 } else { r << 1 };
            }
            *e = r;
        }
        t
    });
    let mut c = 0u32;
    for &b in data {
        c = (c << 8) ^ t[((c >> 24) as u8 ^ b) as usize];
    }
    c
}

/// The most bytes held while waiting for a page to complete (a page is at most 65307 bytes).
const MAX_HOLD: usize = 1 << 20;
/// The largest packet put together.
pub const MAX_PACKET: usize = 1 << 20;

pub struct Demuxer {
    buf: Vec<u8>,
    carry: Vec<u8>,
    /// whether `carry` is a packet that continues onto the next page
    continuing: bool,
    last_seq: Option<(u32, u32)>,
    pub skipped_bytes: u64,
}

impl Default for Demuxer {
    fn default() -> Self {
        Demuxer::new()
    }
}

impl Demuxer {
    pub fn new() -> Demuxer {
        Demuxer { buf: vec![], carry: vec![], continuing: false, last_seq: None, skipped_bytes: 0 }
    }

    /// Feeds bytes; returns every page completed by them.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Page> {
        self.buf.extend_from_slice(bytes);
        let mut pages = vec![];
        let mut at = 0usize;
        loop {
            let rest = &self.buf[at..];
            // find the capture pattern
            let Some(p) = find(rest, b"OggS") else {
                // keep the last three bytes: they may start the pattern
                let keep = rest.len().min(3);
                self.skipped_bytes += (rest.len() - keep) as u64;
                at = self.buf.len() - keep;
                break;
            };
            self.skipped_bytes += p as u64;
            at += p;
            let rest = &self.buf[at..];
            if rest.len() < 27 {
                break;
            }
            let nseg = rest[26] as usize;
            if rest.len() < 27 + nseg {
                break;
            }
            let body: usize = rest[27..27 + nseg].iter().map(|&b| b as usize).sum();
            let total = 27 + nseg + body;
            if rest.len() < total {
                break;
            }
            let ok = rest[4] == 0 && {
                let mut copy = rest[..total].to_vec();
                let want = u32::from_le_bytes([copy[22], copy[23], copy[24], copy[25]]);
                copy[22..26].fill(0);
                crc(&copy) == want
            };
            if !ok {
                // not a real page: look on from the next byte
                self.skipped_bytes += 1;
                at += 1;
                continue;
            }
            let flags = rest[5];
            let granule = i64::from_le_bytes(rest[6..14].try_into().unwrap_or([0; 8]));
            let serial = u32::from_le_bytes(rest[14..18].try_into().unwrap_or([0; 4]));
            let seq = u32::from_le_bytes(rest[18..22].try_into().unwrap_or([0; 4]));
            let table = rest[27..27 + nseg].to_vec();
            let data = rest[27 + nseg..total].to_vec();
            at += total;
            // a gap in the sequence means lost pages: a half packet cannot be finished
            if let Some((s, q)) = self.last_seq {
                if s == serial && seq != q.wrapping_add(1) {
                    self.carry.clear();
                    self.continuing = false;
                }
            }
            self.last_seq = Some((serial, seq));
            if flags & 1 == 0 && self.continuing {
                // this page does not continue the packet that was waiting
                self.carry.clear();
                self.continuing = false;
            }
            let mut packets = vec![];
            let mut off = 0usize;
            for (i, &l) in table.iter().enumerate() {
                let l = l as usize;
                if self.carry.len() + l <= MAX_PACKET {
                    self.carry.extend_from_slice(&data[off..off + l]);
                } else {
                    self.carry.clear();
                }
                off += l;
                if l < 255 {
                    packets.push(std::mem::take(&mut self.carry));
                    self.continuing = false;
                } else {
                    self.continuing = true;
                }
                let _ = i;
            }
            pages.push(Page { serial, seq, bos: flags & 2 != 0, eos: flags & 4 != 0, granule: if packets.is_empty() { -1 } else { granule }, packets });
        }
        self.buf.drain(..at);
        if self.buf.len() > MAX_HOLD {
            self.skipped_bytes += self.buf.len() as u64;
            self.buf.clear();
        }
        pages
    }
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Makes a page by hand: the packets (each whole), the flags and the granule.
    pub fn page(serial: u32, seq: u32, flags: u8, granule: i64, packets: &[&[u8]]) -> Vec<u8> {
        let mut table = vec![];
        let mut body = vec![];
        for p in packets {
            let mut n = p.len();
            while n >= 255 {
                table.push(255);
                n -= 255;
            }
            table.push(n as u8);
            body.extend_from_slice(p);
        }
        page_raw(serial, seq, flags, granule, &table, &body)
    }

    /// A page with the lacing table written out (a lacing value of 255 last means the packet goes on in the next page).
    pub fn page_raw(serial: u32, seq: u32, flags: u8, granule: i64, table: &[u8], body: &[u8]) -> Vec<u8> {
        let mut v = b"OggS".to_vec();
        v.push(0);
        v.push(flags);
        v.extend_from_slice(&granule.to_le_bytes());
        v.extend_from_slice(&serial.to_le_bytes());
        v.extend_from_slice(&seq.to_le_bytes());
        v.extend_from_slice(&[0; 4]);
        v.push(table.len() as u8);
        v.extend_from_slice(table);
        v.extend_from_slice(body);
        let c = crc(&v);
        v[22..26].copy_from_slice(&c.to_le_bytes());
        v
    }

    #[test]
    fn pages_come_out_whole_however_the_bytes_are_cut_and_junk_is_skipped() {
        let mut s = b"junk before".to_vec();
        s.extend(page(7, 0, 2, 0, &[b"hello", b"world!"]));
        s.extend(b"more junk OggS but not a page");
        s.extend(page(7, 1, 0, 100, &[&[9u8; 300]]));
        s.extend(page(7, 2, 4, 200, &[b"end"]));
        for cut in [1usize, 2, 7, 64, 1000] {
            let mut d = Demuxer::new();
            let mut pages = vec![];
            for c in s.chunks(cut) {
                pages.extend(d.push(c));
            }
            assert_eq!(pages.len(), 3, "cut {cut}");
            assert_eq!(pages[0].packets, vec![b"hello".to_vec(), b"world!".to_vec()]);
            assert!(pages[0].bos && pages[2].eos);
            assert_eq!(pages[1].packets[0].len(), 300);
            assert_eq!(pages[1].granule, 100);
        }
    }

    #[test]
    fn a_damaged_page_is_dropped_not_guessed() {
        let mut p = page(1, 0, 2, 0, &[b"abcdef"]);
        let n = p.len();
        p[n - 1] ^= 0x40;
        let mut s = p;
        s.extend(page(1, 1, 0, 5, &[b"good"]));
        let pages = Demuxer::new().push(&s);
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].packets, vec![b"good".to_vec()]);
    }

    #[test]
    fn a_packet_over_two_pages_is_put_together_and_a_lost_page_drops_it() {
        // 255 bytes exactly continue onto the next page (a lacing value of 255 means "more follows")
        let mut a = page_raw(3, 0, 0, -1, &[255], &[1u8; 255]);
        let b = page(3, 1, 1, 10, &[&[2u8; 10]]);
        a.extend(&b);
        let pages = Demuxer::new().push(&a);
        let all: Vec<Vec<u8>> = pages.iter().flat_map(|p| p.packets.clone()).collect();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].len(), 265);
        // page 1 missing: the continuation is not glued to nothing
        let mut c = page_raw(3, 0, 0, -1, &[255], &[1u8; 255]);
        c.extend(page(3, 5, 1, 10, &[&[2u8; 10]]));
        let pages = Demuxer::new().push(&c);
        let all: Vec<Vec<u8>> = pages.iter().flat_map(|p| p.packets.clone()).collect();
        assert!(all.iter().all(|p| p.len() != 265));
    }

    #[test]
    fn random_bytes_and_cut_pages_never_panic() {
        let mut x = 12345u64;
        for n in [0usize, 1, 5, 27, 100, 5000] {
            let v: Vec<u8> = (0..n).map(|_| { x = x.wrapping_mul(6364136223846793005).wrapping_add(1); (x >> 33) as u8 }).collect();
            let _ = Demuxer::new().push(&v);
        }
        let p = page(1, 0, 2, 0, &[b"abc"]);
        for cut in 0..p.len() {
            let _ = Demuxer::new().push(&p[..cut]);
        }
        let mut huge = b"OggS".to_vec();
        huge.extend(std::iter::repeat_n(255u8, 2_000_000));
        let _ = Demuxer::new().push(&huge);
    }
}
