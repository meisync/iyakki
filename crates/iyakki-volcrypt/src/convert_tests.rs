use crate::container::{MemStore, Store, Volume};
use crate::convert::*;
use crate::header::SlotKind;
use crate::header_tests::{TestRandom, CHEAP};
use crate::VolError;

const PW: &[u8] = b"home words";

fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * 7 + i / 512 * 13) % 251) as u8).collect()
}

/// A plain partition: `plain` bytes of data at the start, `gap + JOURNAL_LEN` free bytes at the end (as the shrink step leaves it).
fn plain_store(plain: usize, gap: u64) -> (MemStore, Vec<u8>) {
    let data = pattern(plain);
    let mut bytes = vec![0xA5u8; plain + gap as usize + JOURNAL_LEN as usize];
    bytes[..plain].copy_from_slice(&data);
    let s = MemStore::new(0);
    *s.bytes.lock().unwrap() = bytes;
    (s, data)
}

fn plan(ss: u32, plain: usize, gap: u64) -> Plan {
    Plan { sector_size: ss, plain_bytes: plain as u64, gap_bytes: gap }
}

fn go(s: &mut MemStore, p: &Plan, seen: &mut Vec<Progress>) -> Result<(), VolError> {
    convert(s, &mut TestRandom(9), p, PW, SlotKind::Person, 1000, CHEAP, &mut |x| seen.push(x))
}

fn read_all(s: MemStore, plain: usize) -> Vec<u8> {
    let v = Volume::open_password(s, PW).expect("the converted partition opens with the password");
    let mut out = vec![0u8; plain];
    v.read_sectors(0, &mut out).unwrap();
    out
}

#[test]
fn a_plain_partition_becomes_a_volume_with_the_same_data_and_a_wiped_journal() {
    for (ss, gap) in [(512u32, 8192u64), (512, 16384), (4096, 8192)] {
        let plain = 64 * 4096;
        let (mut s, data) = plain_store(plain, gap);
        assert_eq!(state(&s).unwrap(), State::NotStarted);
        let mut seen = vec![];
        go(&mut s, &plan(ss, plain, gap), &mut seen).unwrap();
        assert_eq!(seen.last().unwrap().moved, seen.last().unwrap().total, "progress ends complete");
        assert!(seen.windows(2).all(|w| w[0].moved < w[1].moved), "progress only grows");
        assert_eq!(state(&s).unwrap(), State::Converted);
        // no run of the plain data is left in the store
        assert!(!s.bytes.lock().unwrap().windows(64).any(|w| w == &data[4096..4160]));
        assert_eq!(read_all(s, plain), data, "ss {ss} gap {gap}");
    }
}

#[test]
fn a_cut_at_any_byte_leaves_the_data_whole_and_the_conversion_resumes_to_the_same_result() {
    for (ss, gap) in [(512u32, 8192u64), (4096, 8192)] {
        let plain = 20 * 4096;
        // how many bytes a clean run writes (the sweep covers all of them)
        let (mut probe, _) = plain_store(plain, gap);
        probe.fail_after = Some(usize::MAX);
        go(&mut probe, &plan(ss, plain, gap), &mut vec![]).unwrap();
        let total = usize::MAX - probe.fail_after.unwrap();
        assert!(total > plain, "{total}");
        let mut cuts: Vec<usize> = (0..total).step_by(1511).collect();
        cuts.extend([1, 4095, 4096, 4097, 8191, 8192, 8193, total - 1]);
        for cut in cuts {
            let (mut s, data) = plain_store(plain, gap);
            s.fail_after = Some(cut);
            let first = go(&mut s, &plan(ss, plain, gap), &mut vec![]);
            assert!(first.is_err(), "cut {cut} of {total} must stop the run");
            s.fail_after = None;
            match state(&s).unwrap() {
                State::NotStarted => {
                    // nothing destructive happened: the plain data is exactly as it was, and a fresh run works
                    assert_eq!(&s.bytes.lock().unwrap()[..plain], &data[..], "cut {cut}");
                    go(&mut s, &plan(ss, plain, gap), &mut vec![]).unwrap();
                }
                State::InProgress { total: t, .. } => {
                    assert_eq!(t, (plain as u64) / ss as u64);
                    resume(&mut s, PW, &mut |_| {}).unwrap_or_else(|e| panic!("cut {cut} (ss {ss}): {e}"));
                }
                State::Finished => finish_cleanup(&mut s).unwrap(),
                State::Converted => {}
            }
            assert_eq!(state(&s).unwrap(), State::Converted, "cut {cut}");
            assert_eq!(read_all(s, plain), data, "cut {cut} (ss {ss}): every sector is back after the resume");
        }
    }
}

#[test]
fn a_second_cut_while_resuming_is_resumed_again() {
    let (ss, gap, plain) = (512u32, 8192u64, 48 * 4096);
    let (mut s, data) = plain_store(plain, gap);
    s.fail_after = Some(plain / 2);
    assert!(go(&mut s, &plan(ss, plain, gap), &mut vec![]).is_err());
    for round in 0..3 {
        s.fail_after = Some(plain / 3);
        let r = resume(&mut s, PW, &mut |_| {});
        if r.is_ok() {
            break;
        }
        assert!(matches!(state(&s).unwrap(), State::InProgress { .. } | State::Finished), "round {round}");
    }
    s.fail_after = None;
    if matches!(state(&s).unwrap(), State::InProgress { .. } | State::Finished) {
        resume(&mut s, PW, &mut |_| {}).unwrap();
    }
    assert_eq!(read_all(s, plain), data);
}

#[test]
fn resuming_needs_the_password_and_changes_nothing_for_a_wrong_one() {
    let (ss, gap, plain) = (512u32, 8192u64, 32 * 4096);
    let (mut s, data) = plain_store(plain, gap);
    s.fail_after = Some(plain);
    assert!(go(&mut s, &plan(ss, plain, gap), &mut vec![]).is_err());
    s.fail_after = None;
    let before = s.bytes.lock().unwrap().clone();
    assert_eq!(resume(&mut s, b"not it", &mut |_| {}).err(), Some(VolError::WrongSecret));
    assert_eq!(*s.bytes.lock().unwrap(), before, "a wrong password writes nothing");
    resume(&mut s, PW, &mut |_| {}).unwrap();
    assert_eq!(read_all(s, plain), data);
}

#[test]
fn what_cannot_be_done_safely_is_refused_before_a_single_write() {
    let (mut s, _) = plain_store(32 * 4096, 8192);
    let before = s.bytes.lock().unwrap().clone();
    let bad = |s: &mut MemStore, p: Plan| go(s, &p, &mut vec![]).err();
    assert!(matches!(bad(&mut s, plan(512, 32 * 4096, 4096)), Some(VolError::BadRequest(_))), "a gap under 8 KiB");
    assert!(matches!(bad(&mut s, plan(512, 32 * 4096, 8192 + 512)), Some(VolError::BadRequest(_))), "a gap that is not a multiple of 4096");
    assert!(matches!(bad(&mut s, plan(1024, 32 * 4096, 8192)), Some(VolError::BadRequest(_))), "a sector size that is not 512 or 4096");
    assert!(matches!(bad(&mut s, plan(512, 32 * 4096 + 100, 8192)), Some(VolError::BadRequest(_))), "data that is not whole sectors");
    assert!(matches!(bad(&mut s, plan(512, 33 * 4096, 8192)), Some(VolError::BadRequest(_))), "no room left for the gap and the journal");
    assert_eq!(*s.bytes.lock().unwrap(), before, "refused means untouched");
    // a finished volume is not converted again
    go(&mut s, &plan(512, 32 * 4096, 8192), &mut vec![]).unwrap();
    assert!(matches!(go(&mut s, &plan(512, 32 * 4096, 8192), &mut vec![]).err(), Some(VolError::BadRequest(_))));
    assert!(matches!(resume(&mut s, PW, &mut |_| {}).err(), Some(VolError::BadRequest(_))), "nothing to resume");
    assert!(matches!(finish_cleanup(&mut s).err(), Some(VolError::BadRequest(_))));
    let _ = s.flush();
}

#[test]
fn the_same_conversion_runs_on_a_real_file_and_a_cut_there_resumes() {
    use crate::container::FileStore;
    let (ss, gap, plain) = (512u32, 8192u64, 16 * 4096);
    let data = pattern(plain);
    let dir = std::env::temp_dir().join(format!("iyakki-volconv-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("partition.img");
    let mut bytes = vec![0u8; plain + gap as usize + JOURNAL_LEN as usize];
    bytes[..plain].copy_from_slice(&data);
    std::fs::write(&path, &bytes).unwrap();
    let open = || FileStore(std::fs::OpenOptions::new().read(true).write(true).open(&path).unwrap());
    // a store that fails after some writes stands in for the power cut; the file keeps what was written
    struct Cut(FileStore, usize);
    impl Store for Cut {
        fn len(&self) -> Result<u64, VolError> {
            self.0.len()
        }
        fn read_at(&self, pos: u64, buf: &mut [u8]) -> Result<(), VolError> {
            self.0.read_at(pos, buf)
        }
        fn write_at(&mut self, pos: u64, buf: &[u8]) -> Result<(), VolError> {
            if self.1 == 0 {
                return Err(VolError::Io("power cut".into()));
            }
            self.1 -= 1;
            self.0.write_at(pos, buf)
        }
        fn flush(&mut self) -> Result<(), VolError> {
            self.0.flush()
        }
    }
    let mut cut = Cut(open(), 9);
    assert!(convert(&mut cut, &mut TestRandom(4), &plan(ss, plain, gap), PW, SlotKind::Person, 1, CHEAP, &mut |_| {}).is_err());
    let mut again = open();
    assert!(matches!(state(&again).unwrap(), State::InProgress { .. }));
    resume(&mut again, PW, &mut |_| {}).unwrap();
    let v = Volume::open_password(open(), PW).unwrap();
    let mut out = vec![0u8; plain];
    v.read_sectors(0, &mut out).unwrap();
    assert_eq!(out, data);
    let _ = std::fs::remove_dir_all(&dir);
}
