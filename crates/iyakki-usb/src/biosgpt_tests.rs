use std::io::Cursor;

use iyakki_storage::gpt::{self, Entry, Table};

use crate::biosgpt::*;
use crate::biosshared::{self, BootCode, Backup};

const MIB: usize = 1 << 20;

fn table(parts: &[(u64, u64)]) -> Table {
    let total = 64 * MIB as u64 / 512;
    let mut t = Table { sector_size: 512, total_sectors: total, disk_guid: gpt::guid_from_str("11111111-2222-3333-4444-555555555555").unwrap(), entries: vec![] };
    for (i, (a, b)) in parts.iter().enumerate() {
        t.entries.push(Entry { index: i as u32 + 1, type_guid: gpt::TYPE_LINUX_DATA, guid: [i as u8 + 1; 16], first_lba: *a, last_lba: *b, attrs: 0, name: format!("p{i}") });
    }
    t
}

fn image(t: &Table) -> Vec<u8> {
    let mut d = vec![0u8; 64 * MIB];
    d[..512].copy_from_slice(&t.protective_mbr(&[], [1, 2, 3, 4]));
    let mut c = Cursor::new(&mut d);
    write_table(&mut c, t).unwrap();
    d
}

#[test]
fn a_gap_before_the_first_partition_gets_an_aligned_one_mib_partition() {
    let t = table(&[(4096, 20000), (20001, 100000)]);
    let p = plan_new_bios_boot(&t, [9; 16]).unwrap();
    assert_eq!((p.entry.first_lba, p.entry.last_lba, p.entry.index), (2048, 4095, 3));
    assert_eq!(p.entry.type_guid, gpt::TYPE_BIOS_BOOT);
    assert!(p.table_after.check().is_ok());
    // the old partitions are exactly as they were
    assert_eq!(&p.table_after.entries[..2], &t.entries[..]);
}

#[test]
fn no_free_mebibyte_or_a_bios_partition_already_there_or_odd_sectors_are_refused_with_the_reason() {
    let t = table(&[(40, 50000), (50001, 131000)]);
    assert!(plan_new_bios_boot(&t, [9; 16]).unwrap_err().contains("no free 1 MiB"));
    let mut t2 = table(&[(4096, 50000)]);
    t2.entries[0].type_guid = gpt::TYPE_BIOS_BOOT;
    assert!(plan_new_bios_boot(&t2, [9; 16]).unwrap_err().contains("already has"));
    let mut t4k = table(&[(4096, 50000)]);
    t4k.sector_size = 4096;
    assert!(plan_new_bios_boot(&t4k, [9; 16]).unwrap_err().contains("4096-byte"));
    let mut bad = table(&[(4096, 50000), (40000, 60000)]);
    bad.entries[1].index = 2;
    assert!(plan_new_bios_boot(&bad, [9; 16]).unwrap_err().contains("not sound"));
}

#[test]
fn a_gap_between_partitions_and_at_the_end_are_used_when_the_front_is_full() {
    let t = table(&[(34, 5000), (9000, 20000)]);
    let p = plan_new_bios_boot(&t, [9; 16]).unwrap();
    assert_eq!(p.entry.first_lba, 6144, "aligned to 1 MiB inside the gap 5001-8999");
    let t = table(&[(34, 100000)]);
    assert_eq!(plan_new_bios_boot(&t, [9; 16]).unwrap().entry.first_lba, 100352);
}

#[test]
fn the_slot_number_is_the_lowest_free_one() {
    let mut t = table(&[(4096, 5000), (6000, 7000), (8000, 9000)]);
    t.entries.remove(1);
    assert_eq!(plan_new_bios_boot(&t, [9; 16]).unwrap().entry.index, 2);
}

#[test]
fn the_new_table_is_saved_first_written_both_ways_read_back_and_restorable() {
    let t = table(&[(4096, 20000), (20001, 100000)]);
    let original = image(&t);
    let p = plan_new_bios_boot(&t, [9; 16]).unwrap();
    let mut disk = Cursor::new(original.clone());
    let save = Backup::take(&mut disk, 512, original.len() as u64, &table_regions(&t)).unwrap();
    write_table(&mut disk, &p.table_after).unwrap();
    // the whole thing reads back through the real reader, both copies agree, and the facts now see an empty BIOS boot partition
    let parsed = gpt::read_gpt(&mut Cursor::new(disk.get_ref().clone()), 512, 64 * MIB as u64 / 512).unwrap();
    assert!(parsed.primary_ok && parsed.backup_ok && parsed.problems.is_empty(), "{:?}", parsed.problems);
    assert_eq!(parsed.table, p.table_after);
    let f = biosshared::read_facts(&mut Cursor::new(disk.get_ref().clone()), 512, original.len() as u64).unwrap();
    assert_eq!(f.bios_boot, Some((2048, 2048)));
    assert!(f.bios_boot_content.iter().all(|b| *b == 0));
    // only the table sectors changed
    let changed: Vec<usize> = original.iter().zip(disk.get_ref()).enumerate().filter(|(_, (a, b))| a != b).map(|(i, _)| i).collect();
    assert!(changed.iter().all(|i| *i < 1024 + 16384 || *i >= (t.backup_entries_lba() as usize) * 512), "something outside the table changed");
    // undo
    save.restore(&mut disk, 512, original.len() as u64).unwrap();
    assert_eq!(disk.get_ref(), &original);
}

#[test]
fn a_power_cut_after_each_write_leaves_one_readable_copy() {
    let t = table(&[(4096, 20000)]);
    let original = image(&t);
    let p = plan_new_bios_boot(&t, [9; 16]).unwrap();
    // writes in order: backup entries, backup header, primary entries, primary header. Stop after each.
    let ss = 512usize;
    let (e, hb, hp) = (p.table_after.entries_bytes(), p.table_after.header_bytes(true), p.table_after.header_bytes(false));
    let steps: [(usize, &[u8]); 4] = [(p.table_after.backup_entries_lba() as usize * ss, &e), (p.table_after.last_lba() as usize * ss, &hb), (2 * ss, &e), (ss, &hp)];
    for cut in 0..=4 {
        let mut d = original.clone();
        for (off, b) in &steps[..cut] {
            d[*off..*off + b.len()].copy_from_slice(b);
        }
        let r = gpt::read_gpt(&mut Cursor::new(d), 512, 64 * MIB as u64 / 512);
        assert!(r.is_ok(), "no readable table after {cut} writes: {r:?}");
    }
}

#[test]
fn the_chainload_entry_uses_only_safe_text_and_replaced_boot_code_needs_a_way_back() {
    let e = chainload_entry("Windows", &Chainload::Partition { uuid: "1A2B-3C4D".into() }).unwrap();
    assert!(e.starts_with("menuentry \"Windows\" {") && e.contains("--fs-uuid --set=root 1A2B-3C4D") && e.contains("chainloader +1"));
    assert!(chainload_entry("Win\"; rm", &Chainload::DiskBootCode).is_err());
    assert!(chainload_entry("W", &Chainload::Partition { uuid: "x; reboot".into() }).is_err());
    assert!(chainload_entry("", &Chainload::DiskBootCode).is_err());
    assert_eq!(entry_for_replaced_boot_code(&BootCode::Empty, None), Ok(None));
    assert_eq!(entry_for_replaced_boot_code(&BootCode::Grub, None), Ok(None));
    assert!(entry_for_replaced_boot_code(&BootCode::Windows, Some("1A2B-3C4D")).unwrap().unwrap().contains("Windows"));
    assert!(entry_for_replaced_boot_code(&BootCode::Windows, None).unwrap_err().contains("unreachable"));
    assert!(entry_for_replaced_boot_code(&BootCode::Other, None).is_err());
}
