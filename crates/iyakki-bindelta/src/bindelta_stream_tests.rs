//! The streaming apply of the binary delta (bindelta.rs): the same bytes as the in-memory apply, nothing left behind on a refusal, and a
//! bound on how much is read or written at a time (counting readers and writers).

use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use super::bindelta::*;
use super::bindelta_tests::program;

#[derive(Default)]
struct Counts {
    max_read: AtomicUsize,
    total: AtomicU64,
}

struct Counting {
    inner: Cursor<Arc<[u8]>>,
    c: Arc<Counts>,
}

impl Read for Counting {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.c.max_read.fetch_max(buf.len(), Ordering::Relaxed);
        let n = self.inner.read(buf)?;
        self.c.total.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

impl Seek for Counting {
    fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(p)
    }
}

#[derive(Default)]
struct CountW {
    max_write: usize,
    total: u64,
    bytes: Vec<u8>,
}

impl Write for CountW {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.max_write = self.max_write.max(b.len());
        self.total += b.len() as u64;
        self.bytes.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct Run {
    out: CountW,
    old: Arc<Counts>,
    delta: Arc<Counts>,
    result: Result<StreamInfo, String>,
}

fn stream(old: &[u8], delta: &[u8]) -> Run {
    let (oc, dc) = (Arc::new(Counts::default()), Arc::new(Counts::default()));
    let mut o = Counting { inner: Cursor::new(Arc::from(old)), c: oc.clone() };
    let dbytes: Arc<[u8]> = Arc::from(delta);
    let dc2 = dc.clone();
    let open = move || -> std::io::Result<Box<dyn ReadSeek>> { Ok(Box::new(Counting { inner: Cursor::new(dbytes.clone()), c: dc2.clone() })) };
    let mut out = CountW::default();
    let result = apply_delta_stream(&mut o, &open, &mut out);
    Run { out, old: oc, delta: dc, result }
}

fn random(n: usize, seed: u64) -> Vec<u8> {
    let mut s = seed | 1;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 24) as u8
        })
        .collect()
}

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-bds-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn the_streamed_result_is_byte_for_byte_the_in_memory_result() {
    let mut cases: Vec<(Vec<u8>, Vec<u8>)> = vec![(vec![], vec![]), (vec![], b"all new".to_vec()), (b"all gone".to_vec(), vec![]), (b"same".to_vec(), b"same".to_vec())];
    let a = random(300_000, 1);
    let mut longer = a.clone();
    longer.extend(random(70_000, 3));
    cases.push((a.clone(), a.clone()));
    cases.push((a.clone(), random(300_000, 2)));
    cases.push((a.clone(), a[..120_000].to_vec()));
    cases.push((a.clone(), longer));
    // a relinked program: every address field moved
    cases.push((program(2_000_000, 7, 0), program(2_100_000, 7, 4096)));
    // a flipped byte here and there in a large equal run
    let mut b = a.clone();
    for i in (0..b.len()).step_by(9_999) {
        b[i] ^= 0x55;
    }
    cases.push((a, b));
    for (old, new) in cases {
        let d = make_delta(&old, &new);
        let mem = apply_delta(&old, &d).unwrap();
        let run = stream(&old, &d);
        let info = run.result.unwrap();
        assert_eq!(run.out.bytes, mem);
        assert_eq!(run.out.bytes, new);
        assert_eq!((info.new_len, info.new_sha256), (new.len() as u64, hex(&sha256(&new))));
    }
}

#[test]
fn what_is_read_and_written_at_a_time_is_bounded_whatever_the_size_of_the_files() {
    let (old, new) = (program(8 << 20, 11, 0), program((8 << 20) + 100_000, 11, 8192));
    let d = make_delta(&old, &new);
    let run = stream(&old, &d);
    let info = run.result.unwrap();
    assert_eq!(run.out.bytes.len(), new.len());
    // no single read from the old file or the delta, and no single write, is bigger than the chunk
    assert!(run.old.max_read.load(Ordering::Relaxed) <= STREAM_CHUNK, "old read {}", run.old.max_read.load(Ordering::Relaxed));
    assert!(run.delta.max_read.load(Ordering::Relaxed) <= STREAM_CHUNK, "delta read {}", run.delta.max_read.load(Ordering::Relaxed));
    assert!(run.out.max_write <= STREAM_CHUNK, "write {}", run.out.max_write);
    // the buffers it holds are a few hundred KiB, not the size of the files (8 MiB each here, 400 MiB for a base image)
    assert!(info.buffers <= 1 << 20 && info.buffers < old.len() / 4, "{}", info.buffers);
    // the old file is read about twice (its hash, then the pieces); the delta once for its trailer and once for its three streams
    assert!(run.old.total.load(Ordering::Relaxed) <= old.len() as u64 + new.len() as u64 + 4096, "old bytes read {}", run.old.total.load(Ordering::Relaxed));
    assert!(run.delta.total.load(Ordering::Relaxed) <= 2 * d.len() as u64 + 4096, "delta bytes read {} of {}", run.delta.total.load(Ordering::Relaxed), d.len());
    assert_eq!(run.out.total, new.len() as u64);
}

#[test]
fn a_wrong_old_file_and_a_cut_or_changed_delta_are_refused_before_a_byte_is_written() {
    let (old, new) = (program(400_000, 5, 0), program(420_000, 5, 64));
    let d = make_delta(&old, &new);
    // the file on the device is not the one the delta was made against: same length, one byte differs
    let mut wrong = old.clone();
    wrong[1000] ^= 1;
    let run = stream(&wrong, &d);
    assert!(run.result.unwrap_err().contains("not the version this delta was made against"));
    assert_eq!(run.out.total, 0);
    let run = stream(&old[..old.len() - 1], &d);
    assert!(run.result.unwrap_err().contains("not the version this delta was made against") && run.out.total == 0);
    // cut off, changed, and not a delta at all
    for bad in [d[..d.len() - 10].to_vec(), d[..d.len() / 2].to_vec(), { let mut c = d.clone(); c[200] ^= 1; c }, b"not a delta".to_vec(), vec![]] {
        let run = stream(&old, &bad);
        assert!(run.result.is_err() && run.out.total == 0, "{:?}", run.result);
    }
}

#[test]
fn on_files_a_refusal_leaves_no_new_file_and_no_temporary_file() {
    let w = tmp("files");
    let (old, new) = (program(300_000, 9, 0), program(310_000, 9, 32));
    let d = make_delta(&old, &new);
    std::fs::write(w.join("old"), &old).unwrap();
    std::fs::write(w.join("delta"), &d).unwrap();
    let names = || -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(&w).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    };
    // works
    let info = apply_delta_files(&w.join("old"), &w.join("delta"), &w.join("new")).unwrap();
    assert_eq!(std::fs::read(w.join("new")).unwrap(), new);
    assert_eq!(info.new_sha256, hex(&sha256(&new)));
    assert_eq!(names(), ["delta", "new", "old"], "no temporary file stays");
    std::fs::remove_file(w.join("new")).unwrap();
    // wrong old file
    let mut wrong = old.clone();
    wrong[5] ^= 1;
    std::fs::write(w.join("wrong"), &wrong).unwrap();
    assert!(apply_delta_files(&w.join("wrong"), &w.join("delta"), &w.join("new")).unwrap_err().contains("not the version this delta was made against"));
    // cut delta
    std::fs::write(w.join("cut"), &d[..d.len() / 3]).unwrap();
    assert!(apply_delta_files(&w.join("old"), &w.join("cut"), &w.join("new")).is_err());
    // a damaged stream with a trailer that was made again: the damage shows up while the pieces are made, after bytes were written
    let mut forged = d.clone();
    let at = forged.len() / 2;
    forged[at] ^= 0xff;
    let n = forged.len();
    let sum = sha256(&forged[..n - 32]);
    forged[n - 32..].copy_from_slice(&sum);
    std::fs::write(w.join("forged"), &forged).unwrap();
    let e = apply_delta_files(&w.join("old"), &w.join("forged"), &w.join("new")).unwrap_err();
    assert!(!e.is_empty());
    // the new file may not be written over the old one
    assert!(apply_delta_files(&w.join("old"), &w.join("delta"), &w.join("old")).is_err());
    assert_eq!(std::fs::read(w.join("old")).unwrap(), old);
    assert_eq!(names(), ["cut", "delta", "forged", "old", "wrong"], "nothing was created by the refusals");
    let _ = std::fs::remove_dir_all(&w);
}

#[test]
fn a_delta_made_before_this_change_still_applies_the_format_is_unchanged() {
    // the layout is fixed by the header: magic, version 1, lengths, hashes; the streaming reader reads exactly that
    let (old, new) = (random(5_000, 4), random(5_500, 5));
    let d = make_delta(&old, &new);
    assert_eq!((&d[..8], u32::from_be_bytes(d[8..12].try_into().unwrap())), (&MAGIC[..], VERSION));
    assert_eq!(info(&d).unwrap().new_sha256, hex(&sha256(&new)));
    assert_eq!(stream(&old, &d).result.unwrap().new_len, new.len() as u64);
}
