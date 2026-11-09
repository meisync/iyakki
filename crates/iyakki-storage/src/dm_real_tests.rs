//! Real-kernel test of the device-mapper layer. NEVER runs by itself: it needs root-level access to `/dev/mapper/control` and
//! `/dev/loop-control`, and the environment variable `IYAKKI_TEST_DM` set (to anything). It works only on a scratch container file in
//! the system temp folder and a mapping named `iyakki-test-dm-<pid>`; no real disk is touched. Skipped (and says so) otherwise.
//!
//!   sudo IYAKKI_TEST_DM=1 cargo test -p iyakki-storage dm_real -- --nocapture     (on a throw-away machine or the QEMU test image)

use super::*;

fn usable() -> bool {
    if std::env::var_os("IYAKKI_TEST_DM").is_none() {
        eprintln!("skipped: IYAKKI_TEST_DM is not set");
        return false;
    }
    let ok = std::fs::OpenOptions::new().read(true).write(true).open("/dev/mapper/control").is_ok();
    if !ok {
        eprintln!("skipped: /dev/mapper/control is not accessible (needs root and the dm-mod kernel option)");
    }
    ok
}

#[test]
fn dm_real_a_container_file_maps_formats_mounts_and_closes() {
    if !usable() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("iyakki-dm-real-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("c.img");
    let f = std::fs::File::create(&file).unwrap();
    f.set_len(64 << 20).unwrap();
    drop(f);
    let k = SysKernel;
    let dev = k.loop_attach(&file).unwrap();
    let name = format!("iyakki-test-dm-{}", std::process::id());
    let key = [0x5au8; KEY_LEN];
    let c = Crypt::new(&k);
    let sectors = (64u64 << 20) / 512 - 16;
    c.open(&name, &key, &dev, 16, sectors, 4096).expect("open the mapping");
    let mp = map_path(&name);
    k.make_fs(&mp, "IYAKKI-TEST").expect("mke2fs");
    let target = dir.join("mnt");
    std::fs::create_dir_all(&target).unwrap();
    k.mount(&mp, &target).expect("mount");
    std::fs::write(target.join("hello.txt"), b"hello").unwrap();
    k.umount(&target).expect("umount");
    c.close(&name).expect("close");
    // what reached the file is not plain
    let raw = std::fs::read(&file).unwrap();
    assert!(!raw.windows(5).any(|w| w == b"hello"));
    k.loop_detach(&dev).expect("detach");
    let _ = std::fs::remove_dir_all(dir);
}
