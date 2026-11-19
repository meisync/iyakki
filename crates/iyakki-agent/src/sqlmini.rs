//! A minimal SQLite file reader and writer for ONE job: leaving some rows out of a browser's cookie database when the profile is sealed
//! (F-VAULT-09, KI-740). It reads every table of a database file (the B-trees, the overflow chains, the records) and writes a new,
//! complete database file from the tables it was given. It is not a database: no SQL runs, nothing is indexed.
//!
//! Honest limits, each of which is a refusal, never a guess: only UTF-8 databases; only tables (an index, a view or a trigger would be
//! lost, so a file that has one is refused); a write-ahead log is taken in first by [`apply_wal`] (the caller hands it over); no auto-vacuum pointer pages; no rows over 64 MiB. The rows are copied as they are (their record bytes), so
//! column types, floats, blobs and NULLs survive exactly. The output is checked by reading it again before it is returned, and the
//! tests open it with the `sqlite3` program where there is one (an independent reader).

use std::collections::BTreeMap;

const MAGIC: &[u8; 16] = b"SQLite format 3\0";
/// The page size of what is written.
const PAGE: usize = 4096;
const MAX_PAGES: usize = 1 << 20;
const MAX_ROW: usize = 64 << 20;

/// One table: its name, its `CREATE TABLE` text and its rows as (rowid, record bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    pub name: String,
    pub sql: String,
    pub rows: Vec<(i64, Vec<u8>)>,
}

fn be16(b: &[u8], o: usize) -> usize {
    u16::from_be_bytes([b[o], b[o + 1]]) as usize
}
fn be32(b: &[u8], o: usize) -> usize {
    u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as usize
}

/// A SQLite varint at `at`: (value, bytes used).
fn varint(b: &[u8], at: usize) -> Option<(u64, usize)> {
    let mut v = 0u64;
    for i in 0..9 {
        let c = *b.get(at + i)?;
        if i == 8 {
            return Some(((v << 8) | c as u64, 9));
        }
        v = (v << 7) | (c & 0x7f) as u64;
        if c & 0x80 == 0 {
            return Some((v, i + 1));
        }
    }
    None
}

fn put_varint(out: &mut Vec<u8>, v: u64) {
    if v > 0x00ff_ffff_ffff_ffff {
        // nine bytes: the first eight carry 7 bits each, the last all 8
        let mut tmp = [0u8; 9];
        tmp[8] = (v & 0xff) as u8;
        let mut x = v >> 8;
        for i in (0..8).rev() {
            tmp[i] = ((x & 0x7f) as u8) | 0x80;
            x >>= 7;
        }
        out.extend_from_slice(&tmp);
        return;
    }
    let mut buf = [0u8; 9];
    let mut n = 0;
    let mut x = v;
    loop {
        buf[n] = (x & 0x7f) as u8;
        x >>= 7;
        n += 1;
        if x == 0 {
            break;
        }
    }
    for i in (0..n).rev() {
        out.push(buf[i] | if i == 0 { 0 } else { 0x80 });
    }
}

struct Db<'a> {
    b: &'a [u8],
    page: usize,
    usable: usize,
}

impl<'a> Db<'a> {
    fn open(b: &'a [u8]) -> Result<Db<'a>, String> {
        if b.len() < 512 || &b[..16] != MAGIC {
            return Err("this is not an SQLite database file".into());
        }
        let ps = be16(b, 16);
        let page = if ps == 1 { 65536 } else { ps };
        if !(512..=65536).contains(&page) || !page.is_power_of_two() {
            return Err("the database has a page size this reader refuses".into());
        }
        if b[18] > 2 || b[19] > 2 {
            return Err("the database is in an unknown format".into());
        }
        if be32(b, 56) != 1 {
            return Err("only UTF-8 databases are read".into());
        }
        if be32(b, 52) != 0 {
            return Err("an auto-vacuum database is not read".into());
        }
        let usable = page - b[20] as usize;
        if usable < 480 || b.len() % page != 0 {
            return Err("the database file is cut or damaged".into());
        }
        Ok(Db { b, page, usable })
    }

    fn pages(&self) -> usize {
        self.b.len() / self.page
    }

    fn page_bytes(&self, n: usize) -> Result<&'a [u8], String> {
        if n == 0 || n > self.pages() {
            return Err(format!("page {n} is outside the file"));
        }
        Ok(&self.b[(n - 1) * self.page..n * self.page])
    }

    /// All (rowid, payload) of the table tree rooted at page `root`.
    fn rows(&self, root: usize) -> Result<Vec<(i64, Vec<u8>)>, String> {
        let mut out = Vec::new();
        let mut seen = 0usize;
        self.walk(root, 0, &mut out, &mut seen)?;
        Ok(out)
    }

    fn walk(&self, n: usize, depth: u32, out: &mut Vec<(i64, Vec<u8>)>, seen: &mut usize) -> Result<(), String> {
        *seen += 1;
        if depth > 40 || *seen > self.pages() + 1 {
            return Err("the database has a loop in its pages".into());
        }
        let p = self.page_bytes(n)?;
        let h = if n == 1 { 100 } else { 0 };
        let kind = p[h];
        let cells = be16(p, h + 3);
        match kind {
            0x0d => {
                for i in 0..cells {
                    let at = be16(p, h + 8 + 2 * i);
                    let (len, a) = varint(p, at).ok_or("a cell is damaged")?;
                    let (rowid, b) = varint(p, at + a).ok_or("a cell is damaged")?;
                    let len = len as usize;
                    if len > MAX_ROW {
                        return Err("a row is too big".into());
                    }
                    let payload = self.payload(p, at + a + b, len)?;
                    out.push((rowid as i64, payload));
                }
                Ok(())
            }
            0x05 => {
                for i in 0..cells {
                    let at = be16(p, h + 12 + 2 * i);
                    if at + 4 > p.len() {
                        return Err("a cell is damaged".into());
                    }
                    self.walk(be32(p, at), depth + 1, out, seen)?;
                }
                self.walk(be32(p, h + 8), depth + 1, out, seen)
            }
            _ => Err("a page of the table is not a table page (an index?)".into()),
        }
    }

    /// The whole payload of a cell whose local part starts at `at` (it follows the overflow chain when there is one).
    fn payload(&self, p: &[u8], at: usize, len: usize) -> Result<Vec<u8>, String> {
        let u = self.usable;
        let x = u - 35;
        if len <= x {
            return p.get(at..at + len).map(|s| s.to_vec()).ok_or_else(|| "a cell is damaged".into());
        }
        let m = ((u - 12) * 32 / 255) - 23;
        let k = m + ((len - m) % (u - 4));
        let local = if k <= x { k } else { m };
        let mut out = p.get(at..at + local).ok_or("a cell is damaged")?.to_vec();
        let mut next = be32(p, at + local);
        let mut hops = 0;
        while out.len() < len {
            hops += 1;
            if next == 0 || hops > self.pages() {
                return Err("an overflow chain is broken".into());
            }
            let o = self.page_bytes(next)?;
            let take = (len - out.len()).min(u - 4);
            out.extend_from_slice(&o[4..4 + take]);
            next = be32(o, 0);
        }
        Ok(out)
    }
}

/// The values of a record, as far as the callers need them: text columns by position.
pub fn column_text(record: &[u8], col: usize) -> Option<String> {
    let (hlen, a) = varint(record, 0)?;
    let hlen = hlen as usize;
    let mut types = Vec::new();
    let mut at = a;
    while at < hlen {
        let (t, n) = varint(record, at)?;
        types.push(t);
        at += n;
    }
    let mut pos = hlen;
    for (i, t) in types.iter().enumerate() {
        let size = serial_size(*t)?;
        if i == col {
            if *t >= 13 && t % 2 == 1 {
                return Some(String::from_utf8_lossy(record.get(pos..pos + size)?).into_owned());
            }
            return None;
        }
        pos += size;
    }
    None
}

fn serial_size(t: u64) -> Option<usize> {
    Some(match t {
        0 | 8 | 9 => 0,
        1 => 1,
        2 => 2,
        3 => 3,
        4 => 4,
        5 => 6,
        6 | 7 => 8,
        10 | 11 => return None,
        n => ((n - 12) / 2) as usize,
    })
}

/// The database as it stands after its write-ahead log (`-wal`) is taken in: every frame of the log up to its last commit is laid over the
/// pages of the file, as a checkpoint would do, in memory (nothing is written). The log is trusted only as far as SQLite trusts it: the
/// header's checksum, the salts and every frame's running checksum must agree, and the frames after the last commit (or after the first
/// frame that does not agree) are ignored. A log that is not one, or a page size that differs from the file's, is refused.
pub fn apply_wal(db: &[u8], wal: &[u8]) -> Result<Vec<u8>, String> {
    if wal.is_empty() {
        return Ok(db.to_vec());
    }
    if wal.len() < 32 {
        return Err("the write-ahead log is too short to be one".into());
    }
    let be32 = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    let magic = be32(&wal[0..4]);
    if magic != 0x377f0682 && magic != 0x377f0683 {
        return Err("the write-ahead log has an unknown header".into());
    }
    let big_endian_sums = magic & 1 == 1;
    let page = be32(&wal[8..12]) as usize;
    if !(512..=65536).contains(&page) || !page.is_power_of_two() {
        return Err("the write-ahead log names a page size no database has".into());
    }
    if db.len() >= 18 {
        let fp = match u16::from_be_bytes([db[16], db[17]]) {
            1 => 65536,
            n => n as usize,
        };
        if &db[..16] != MAGIC || fp != page {
            return Err("the write-ahead log does not belong to this database file (page size or header differ)".into());
        }
    }
    let sum = |mut s: (u32, u32), data: &[u8]| -> (u32, u32) {
        for w in data.chunks_exact(8) {
            let (x0, x1) = if big_endian_sums { (be32(&w[0..4]), be32(&w[4..8])) } else { (u32::from_le_bytes([w[0], w[1], w[2], w[3]]), u32::from_le_bytes([w[4], w[5], w[6], w[7]])) };
            s.0 = s.0.wrapping_add(x0).wrapping_add(s.1);
            s.1 = s.1.wrapping_add(x1).wrapping_add(s.0);
        }
        s
    };
    let head = sum((0, 0), &wal[0..24]);
    if head != (be32(&wal[24..28]), be32(&wal[28..32])) {
        return Err("the write-ahead log header does not check out".into());
    }
    let (salt1, salt2) = (be32(&wal[16..20]), be32(&wal[20..24]));
    let mut running = head;
    let mut pending: Vec<(usize, &[u8])> = Vec::new();
    let mut committed: Vec<(usize, &[u8])> = Vec::new();
    let mut size_pages = db.len() / page;
    let mut at = 32;
    while at + 24 + page <= wal.len() {
        let fh = &wal[at..at + 24];
        let data = &wal[at + 24..at + 24 + page];
        if be32(&fh[8..12]) != salt1 || be32(&fh[12..16]) != salt2 {
            break;
        }
        let s = sum(sum(running, &fh[0..8]), data);
        if s != (be32(&fh[16..20]), be32(&fh[20..24])) {
            break;
        }
        running = s;
        let pgno = be32(&fh[0..4]) as usize;
        if pgno == 0 || pgno > MAX_PAGES {
            return Err("the write-ahead log names a page that cannot exist".into());
        }
        pending.push((pgno, data));
        let commit = be32(&fh[4..8]) as usize;
        if commit != 0 {
            committed.append(&mut pending);
            size_pages = commit;
        }
        at += 24 + page;
    }
    if committed.is_empty() {
        return Ok(db.to_vec());
    }
    if size_pages == 0 || size_pages > MAX_PAGES {
        return Err("the write-ahead log gives a database size that cannot be".into());
    }
    let mut out = db.to_vec();
    out.resize(size_pages * page, 0);
    for (pgno, data) in committed {
        if pgno <= size_pages {
            out[(pgno - 1) * page..pgno * page].copy_from_slice(data);
        }
    }
    Ok(out)
}

/// Read every table of a database file. A file that holds anything else than tables is refused (it would be lost in a rewrite).
pub fn read(db: &[u8]) -> Result<Vec<Table>, String> {
    let d = Db::open(db)?;
    let master = d.rows(1)?;
    let mut tables = Vec::new();
    for (_, rec) in &master {
        let kind = column_text(rec, 0).ok_or("the schema is damaged")?;
        let name = column_text(rec, 1).ok_or("the schema is damaged")?;
        match kind.as_str() {
            "table" => {
                if name == "sqlite_sequence" || name.starts_with("sqlite_stat") {
                    return Err("the database has bookkeeping tables this rewrite would lose".into());
                }
                let root = master_root(rec).ok_or("the schema is damaged")?;
                let sql = column_text(rec, 4).ok_or("the schema is damaged")?;
                if sql.to_ascii_uppercase().contains("VIRTUAL TABLE") || sql.to_ascii_uppercase().contains("WITHOUT ROWID") {
                    return Err("a virtual or WITHOUT ROWID table is not rewritten".into());
                }
                let rows = d.rows(root)?;
                tables.push(Table { name, sql, rows });
            }
            _ => return Err(format!("the database holds a {kind} ({name}), which a rewrite would lose")),
        }
    }
    Ok(tables)
}

/// The `rootpage` integer of a schema record (column 3).
fn master_root(rec: &[u8]) -> Option<usize> {
    let (hlen, a) = varint(rec, 0)?;
    let hlen = hlen as usize;
    let mut types = Vec::new();
    let mut at = a;
    while at < hlen {
        let (t, n) = varint(rec, at)?;
        types.push(t);
        at += n;
    }
    let mut pos = hlen;
    for (i, t) in types.iter().enumerate() {
        let size = serial_size(*t)?;
        if i == 3 {
            return match *t {
                8 => Some(0),
                9 => Some(1),
                1..=4 => {
                    let mut v = 0i64;
                    for (k, byte) in rec.get(pos..pos + size)?.iter().enumerate() {
                        v = if k == 0 { *byte as i8 as i64 } else { (v << 8) | *byte as i64 };
                    }
                    usize::try_from(v).ok()
                }
                _ => None,
            };
        }
        pos += size;
    }
    None
}

fn text_value(out: &mut Vec<u8>, types: &mut Vec<u8>, s: &str) {
    put_varint(types, 13 + 2 * s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

fn schema_record(name: &str, root: usize, sql: &str) -> Vec<u8> {
    let (mut types, mut body) = (Vec::new(), Vec::new());
    text_value(&mut body, &mut types, "table");
    text_value(&mut body, &mut types, name);
    text_value(&mut body, &mut types, name);
    // the root page number as the smallest integer type that holds it
    if root < 128 {
        put_varint(&mut types, 1);
        body.push(root as u8);
    } else if root < 32768 {
        put_varint(&mut types, 2);
        body.extend_from_slice(&(root as u16).to_be_bytes());
    } else {
        put_varint(&mut types, 4);
        body.extend_from_slice(&(root as u32).to_be_bytes());
    }
    text_value(&mut body, &mut types, sql);
    let mut rec = Vec::new();
    put_varint(&mut rec, types.len() as u64 + 1);
    rec.extend_from_slice(&types);
    rec.extend_from_slice(&body);
    rec
}

struct Builder {
    pages: Vec<Vec<u8>>,
}

impl Builder {
    /// A new page; returns its number (1-based). Page 1 is reserved first.
    fn push(&mut self, p: Vec<u8>) -> Result<usize, String> {
        if self.pages.len() >= MAX_PAGES {
            return Err("the database would be too big".into());
        }
        self.pages.push(p);
        Ok(self.pages.len())
    }

    /// One cell of a leaf: payload size, rowid, the local part, and an overflow chain when the payload does not fit.
    fn leaf_cell(&mut self, rowid: i64, payload: &[u8]) -> Result<Vec<u8>, String> {
        let u = PAGE;
        let x = u - 35;
        let mut cell = Vec::new();
        put_varint(&mut cell, payload.len() as u64);
        put_varint(&mut cell, rowid as u64);
        if payload.len() <= x {
            cell.extend_from_slice(payload);
            return Ok(cell);
        }
        let m = ((u - 12) * 32 / 255) - 23;
        let k = m + ((payload.len() - m) % (u - 4));
        let local = if k <= x { k } else { m };
        cell.extend_from_slice(&payload[..local]);
        // the rest goes to overflow pages, chained forward: each page is written with the number of the next one known in advance
        let rest = &payload[local..];
        let chunks: Vec<&[u8]> = rest.chunks(u - 4).collect();
        let first = self.pages.len() + 1;
        cell.extend_from_slice(&(first as u32).to_be_bytes());
        for (i, c) in chunks.iter().enumerate() {
            let mut p = vec![0u8; PAGE];
            let next = if i + 1 < chunks.len() { (first + i + 1) as u32 } else { 0 };
            p[..4].copy_from_slice(&next.to_be_bytes());
            p[4..4 + c.len()].copy_from_slice(c);
            self.push(p)?;
        }
        Ok(cell)
    }
}

fn finish_page(kind: u8, header_at: usize, cells: &[Vec<u8>], right: Option<u32>) -> Vec<u8> {
    let mut p = vec![0u8; PAGE];
    let hdr = if kind == 0x05 { 12 } else { 8 };
    let mut top = PAGE;
    for (i, c) in cells.iter().enumerate() {
        top -= c.len();
        p[top..top + c.len()].copy_from_slice(c);
        let ptr = header_at + hdr + 2 * i;
        p[ptr..ptr + 2].copy_from_slice(&(top as u16).to_be_bytes());
    }
    p[header_at] = kind;
    p[header_at + 3..header_at + 5].copy_from_slice(&(cells.len() as u16).to_be_bytes());
    p[header_at + 5..header_at + 7].copy_from_slice(&((if cells.is_empty() { PAGE } else { top }) as u16).to_be_bytes());
    if let Some(r) = right {
        p[header_at + 8..header_at + 12].copy_from_slice(&r.to_be_bytes());
    }
    p
}

/// Build the B-tree of one table into `b`; returns the number of its root page.
fn build_table(b: &mut Builder, rows: &[(i64, Vec<u8>)]) -> Result<usize, String> {
    let mut rows: Vec<&(i64, Vec<u8>)> = rows.iter().collect();
    rows.sort_by_key(|r| r.0);
    if rows.first().is_some_and(|r| r.0 < 0) {
        return Err("a negative row number is not written".into());
    }
    let mut level: Vec<(usize, i64)> = Vec::new(); // (page number, largest rowid under it)
    let mut cells: Vec<Vec<u8>> = Vec::new();
    let mut used = 0usize;
    let mut last = 0i64;
    for (rowid, payload) in rows {
        let cell = b.leaf_cell(*rowid, payload)?;
        if used + cell.len() + 2 > PAGE - 8 && !cells.is_empty() {
            let n = b.push(finish_page(0x0d, 0, &cells, None))?;
            level.push((n, last));
            cells.clear();
            used = 0;
        }
        used += cell.len() + 2;
        last = *rowid;
        cells.push(cell);
    }
    let n = b.push(finish_page(0x0d, 0, &cells, None))?;
    level.push((n, last));
    // interior levels until one page is left: a page holds as many children as fit (a cell is a 4-byte child number and a key of at most 9
    // bytes), the children are shared out evenly so that no page is left with one child, and every leaf stays at the same depth
    while level.len() > 1 {
        let cap = (PAGE - 12) / (2 + 4 + 9) + 1;
        let groups = level.len().div_ceil(cap);
        let size = level.len().div_ceil(groups);
        let mut next: Vec<(usize, i64)> = Vec::new();
        for group in level.chunks(size) {
            let (right, max) = *group.last().expect("a chunk has a child");
            let mut cs = Vec::new();
            for (child, key) in &group[..group.len() - 1] {
                let mut c = (*child as u32).to_be_bytes().to_vec();
                put_varint(&mut c, *key as u64);
                cs.push(c);
            }
            let n = b.push(finish_page(0x05, 0, &cs, Some(right as u32)))?;
            next.push((n, max));
        }
        level = next;
    }
    Ok(level[0].0)
}

/// Write a complete database file holding these tables.
pub fn write(tables: &[Table]) -> Result<Vec<u8>, String> {
    let mut b = Builder { pages: vec![vec![0u8; PAGE]] };
    let mut master: Vec<(String, usize, String)> = Vec::new();
    for t in tables {
        if t.name.is_empty() || t.sql.is_empty() {
            return Err("a table has no name or no definition".into());
        }
        let root = build_table(&mut b, &t.rows)?;
        master.push((t.name.clone(), root, t.sql.clone()));
    }
    // page 1: the 100-byte header, then the schema table as a leaf
    let mut cells = Vec::new();
    let mut used = 0;
    for (i, (name, root, sql)) in master.iter().enumerate() {
        let rec = schema_record(name, *root, sql);
        let mut cell = Vec::new();
        put_varint(&mut cell, rec.len() as u64);
        put_varint(&mut cell, i as u64 + 1);
        cell.extend_from_slice(&rec);
        if cell.len() > PAGE - 35 {
            return Err("a table definition is too long".into());
        }
        used += cell.len() + 2;
        cells.push(cell);
    }
    if used + 100 + 8 > PAGE {
        return Err("too many tables for one schema page".into());
    }
    let mut p1 = finish_page(0x0d, 100, &cells, None);
    p1[..16].copy_from_slice(MAGIC);
    p1[16..18].copy_from_slice(&(PAGE as u16).to_be_bytes());
    p1[18] = 1;
    p1[19] = 1;
    p1[21] = 64;
    p1[22] = 32;
    p1[23] = 32;
    p1[24..28].copy_from_slice(&1u32.to_be_bytes());
    p1[28..32].copy_from_slice(&(b.pages.len() as u32).to_be_bytes());
    p1[40..44].copy_from_slice(&1u32.to_be_bytes());
    p1[44..48].copy_from_slice(&4u32.to_be_bytes());
    p1[56..60].copy_from_slice(&1u32.to_be_bytes());
    p1[92..96].copy_from_slice(&1u32.to_be_bytes());
    p1[96..100].copy_from_slice(&3_045_000u32.to_be_bytes());
    b.pages[0] = p1;
    let mut out = Vec::with_capacity(b.pages.len() * PAGE);
    for p in &b.pages {
        out.extend_from_slice(p);
    }
    // what was written must read back as what was given
    let back = read(&out).map_err(|e| format!("the rewritten database does not read back: {e}"))?;
    let mut want: BTreeMap<&str, Vec<(i64, &Vec<u8>)>> = BTreeMap::new();
    for t in tables {
        let mut r: Vec<(i64, &Vec<u8>)> = t.rows.iter().map(|(i, v)| (*i, v)).collect();
        r.sort_by_key(|x| x.0);
        want.insert(&t.name, r);
    }
    for t in &back {
        let mut r: Vec<(i64, &Vec<u8>)> = t.rows.iter().map(|(i, v)| (*i, v)).collect();
        r.sort_by_key(|x| x.0);
        if want.get(t.name.as_str()) != Some(&r) {
            return Err(format!("the rewritten table {} does not read back as written", t.name));
        }
    }
    if back.len() != tables.len() {
        return Err("the rewritten database has another number of tables".into());
    }
    Ok(out)
}
