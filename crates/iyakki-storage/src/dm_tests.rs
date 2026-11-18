use super::fake::FakeKernel;
use super::*;
use std::path::PathBuf;

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-dm-{tag}-{}-{:?}", std::process::id(), std::thread::current().id()).replace(['(', ')'], ""));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn key() -> [u8; KEY_LEN] {
    let mut k = [0u8; KEY_LEN];
    for (i, b) in k.iter_mut().enumerate() {
        *b = (i as u8).wrapping_mul(7).wrapping_add(3);
    }
    k
}

fn hex(k: &[u8]) -> String {
    k.iter().map(|b| format!("{b:02x}")).collect()
}

fn setup(tag: &str) -> (PathBuf, FakeKernel, String) {
    let d = tmp(tag);
    let k = FakeKernel::new(&d.join("fs"));
    let f = d.join("home.ivol");
    std::fs::write(&f, vec![0u8; 16384]).unwrap();
    let dev = k.loop_attach(&f).unwrap();
    (d, k, dev)
}

#[test]
fn request_numbers_are_the_kernels() {
    assert_eq!(DmCmd::Version.request(), 0xc138fd00);
    assert_eq!(DmCmd::DevCreate.request(), 0xc138fd03);
    assert_eq!(DmCmd::DevRemove.request(), 0xc138fd04);
    assert_eq!(DmCmd::DevSuspend.request(), 0xc138fd06);
    assert_eq!(DmCmd::DevStatus.request(), 0xc138fd07);
    assert_eq!(DmCmd::TableLoad.request(), 0xc138fd09);
    for c in [DmCmd::Version, DmCmd::DevCreate, DmCmd::DevRemove, DmCmd::DevSuspend, DmCmd::DevStatus, DmCmd::TableLoad] {
        assert_eq!(DmCmd::from_request(c.request()), Some(c));
    }
}

#[test]
fn the_table_line_has_the_designed_format() {
    let k = hex(&key());
    let p = crypt_params(&k, "/dev/loop7", 16, 4096).unwrap();
    assert_eq!(full_table_line(2048, &p), format!("0 2048 crypt aes-xts-plain64 {k} 0 /dev/loop7 16 1 sector_size:4096"));
    // 512-byte volumes carry no optional parameter (dm-crypt's own default)
    assert_eq!(crypt_params(&k, "/dev/loop7", 16, 512).unwrap(), format!("aes-xts-plain64 {k} 0 /dev/loop7 16"));
    assert!(crypt_params(&k, "/dev/loop7", 16, 1024).is_err());
    assert!(crypt_params(&k, "", 16, 4096).is_err());
    assert!(crypt_params(&k, "/dev/loop7 evil", 16, 4096).is_err(), "a space would add parameters");
}

#[test]
fn table_buffers_are_laid_out_like_the_kernel_reads_them() {
    let b = table_load_buf("iyakki-home-amy", 4096, "crypt", "x y z", DM_SECURE_DATA_FLAG);
    assert_eq!(get_u32(&b, 0), 4);
    assert_eq!(get_u32(&b, 12) as usize, b.len());
    assert_eq!(get_u32(&b, 16), 312);
    assert_eq!(get_u32(&b, 20), 1);
    assert_eq!(b.len() % 8, 0);
    assert_eq!(get_u32(&b, 28), DM_SECURE_DATA_FLAG);
    let t = decode_table_load(&b).unwrap();
    assert_eq!((t.start, t.sectors, t.target_type.as_str(), t.params.as_str(), t.name.as_str()), (0, 4096, "crypt", "x y z", "iyakki-home-amy"));
    // hostile or damaged buffers are refused
    let mut bad = b.clone();
    bad[12] ^= 1;
    assert!(decode_table_load(&bad).is_err());
    assert!(decode_table_load(&b[..b.len() - 8]).is_err());
    assert!(decode_table_load(&b[..100]).is_err());
    let mut unterminated = b.clone();
    let n = unterminated.len();
    unterminated[n - 8..].fill(b'a');
    assert!(decode_table_load(&unterminated).is_err());
}

#[test]
fn opening_creates_loads_resumes_in_that_order_and_hands_over_the_key_once() {
    let (d, k, dev) = setup("open");
    let c = Crypt::new(&k);
    c.open("iyakki-home-amy", &key(), &dev, 16, 2048, 4096).unwrap();
    assert_eq!(k.log(), vec!["loop:attach /dev/loop0", "dm:create iyakki-home-amy", "dm:table iyakki-home-amy", "dm:resume iyakki-home-amy"]);
    let t = &k.tables()[0];
    assert_eq!(t.flags & DM_SECURE_DATA_FLAG, DM_SECURE_DATA_FLAG);
    assert_eq!(full_table_line(t.sectors, &t.params), format!("0 2048 crypt aes-xts-plain64 {} 0 /dev/loop0 16 1 sector_size:4096", hex(&key())));
    let st = c.status("iyakki-home-amy").unwrap().unwrap();
    assert!(st.active && st.open_count == 0);
    c.close("iyakki-home-amy").unwrap();
    assert_eq!(c.status("iyakki-home-amy").unwrap(), None);
    k.loop_detach(&dev).unwrap();
    assert!(k.loop_devices().is_empty() && k.mapping_names().is_empty());
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn the_key_is_in_no_log_line_error_or_debug_text() {
    let (d, k, dev) = setup("nolog");
    let c = Crypt::new(&k);
    let h = hex(&key());
    k.fail_once("dm:table");
    let e = c.open("iyakki-home-amy", &key(), &dev, 16, 2048, 4096).unwrap_err();
    assert!(!format!("{e} {e:?}").contains(&h));
    c.open("iyakki-home-amy", &key(), &dev, 16, 2048, 4096).unwrap();
    for l in k.log() {
        assert!(!l.contains(&h) && !l.contains(&h[..32]), "key in log: {l}");
    }
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_failed_open_leaves_no_mapping_behind() {
    for step in ["dm:table", "dm:resume"] {
        let (d, k, dev) = setup("fail");
        let c = Crypt::new(&k);
        k.fail_once(step);
        assert!(c.open("iyakki-home-amy", &key(), &dev, 16, 2048, 4096).is_err());
        assert!(k.mapping_names().is_empty(), "{step}: mapping left behind");
        // and a second try works
        c.open("iyakki-home-amy", &key(), &dev, 16, 2048, 4096).unwrap();
        let _ = std::fs::remove_dir_all(d);
    }
    let (d, k, dev) = setup("failcreate");
    k.fail_once("dm:create");
    assert!(Crypt::new(&k).open("iyakki-home-amy", &key(), &dev, 16, 2048, 4096).is_err());
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn bad_names_sizes_and_devices_are_refused_before_the_kernel_is_asked() {
    let (d, k, dev) = setup("refuse");
    let c = Crypt::new(&k);
    for n in ["", "A", "a b", "../x", "a/b", &"a".repeat(65)] {
        assert_eq!(c.open(n, &key(), &dev, 16, 2048, 4096), Err(DmError::BadName), "{n}");
        assert_eq!(c.close(n), Err(DmError::BadName));
    }
    assert!(c.open("ok", &key(), &dev, 3, 2048, 4096).is_err(), "offset not a whole 4096 unit");
    assert!(c.open("ok", &key(), &dev, 16, 2047, 4096).is_err());
    assert!(c.open("ok", &key(), &dev, 16, 0, 4096).is_err());
    assert!(c.open("ok", &key(), &dev, 16, 2048, 1024).is_err());
    assert!(c.open("ok", &key(), "/dev/loop0 x", 16, 2048, 4096).is_err());
    assert!(!k.log().iter().any(|l| l.starts_with("dm:")), "the kernel must not have been asked");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn the_fake_refuses_what_the_kernel_would() {
    let (d, k, dev) = setup("kernelrules");
    let c = Crypt::new(&k);
    // a table for a device that is not attached
    assert!(c.open("ok", &key(), "/dev/loop99", 16, 2048, 4096).is_err());
    assert!(k.mapping_names().is_empty());
    // the same name twice
    c.open("ok", &key(), &dev, 16, 2048, 4096).unwrap();
    assert!(c.open("ok", &key(), &dev, 16, 2048, 4096).is_err());
    // the loop device is busy while the mapping sits on it
    assert!(k.loop_detach(&dev).is_err());
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn make_mount_unmount_and_a_busy_mapping_cannot_be_closed() {
    let (d, k, dev) = setup("mount");
    let c = Crypt::new(&k);
    c.open("iyakki-home-amy", &key(), &dev, 16, 2048, 4096).unwrap();
    let dm = map_path("iyakki-home-amy");
    let target = d.join("home");
    std::fs::create_dir_all(&target).unwrap();
    assert!(k.mount(&dm, &target).is_err(), "no file system yet");
    k.make_fs(&dm, "IYAKKI-HOME").unwrap();
    k.mount(&dm, &target).unwrap();
    assert!(k.is_mounted(&target));
    std::fs::write(target.join("note.txt"), b"private").unwrap();
    assert_eq!(c.status("iyakki-home-amy").unwrap().unwrap().open_count, 1);
    assert!(c.close("iyakki-home-amy").is_err(), "mounted: busy");
    k.umount(&target).unwrap();
    assert!(!target.join("note.txt").exists(), "unmounted: the content is out of view");
    k.mount(&dm, &target).unwrap();
    assert_eq!(std::fs::read(target.join("note.txt")).unwrap(), b"private");
    k.umount(&target).unwrap();
    c.close("iyakki-home-amy").unwrap();
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn wipe_zeroes_a_buffer() {
    let mut b = vec![0xAAu8; 100];
    wipe(&mut b);
    assert!(b.iter().all(|x| *x == 0));
}
