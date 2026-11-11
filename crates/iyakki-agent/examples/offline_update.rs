// Throwaway: applies an update pack to an IYAKKI stick that is mounted on this computer, with the device's own update code.
// usage: offline_update <data-mount> <boot-mount> <pack-dir> <release-text-file>
use std::path::PathBuf;
use iyakki_agent::devmode::update::*;
use iyakki_agent::devmode::DevMode;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (data, boot, pack, rel) = (PathBuf::from(&a[1]), PathBuf::from(&a[2]), PathBuf::from(&a[3]), std::fs::read_to_string(&a[4]).unwrap());
    let m = read_manifest(&pack).expect("manifest");
    verify_payload(&pack, &m).expect("payload does not match its manifest");
    let dm = DevMode::open(&data);
    let b = DirBoot { root: boot.clone(), capacity: 118030u64 * 4096, cluster: 4096 };
    let free = || { let s = std::process::Command::new("df").args(["-B1", "--output=avail"]).arg(&data).output().unwrap(); String::from_utf8_lossy(&s.stdout).lines().nth(1).unwrap().trim().parse::<u64>().unwrap() };
    let env = Env { dm: &dm, boot: &b, release: rel, volatile: false, data_free: &free, fault_after: None };
    let p = plan(&env, &m, true).expect("plan");
    println!("plan: {p:?}");
    for n in &p.needs {
        let dst = staged_path(&env, n);
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        let src = find_ci(&pack, n).unwrap_or_else(|| pack.join(n));
        std::fs::copy(&src, &dst).unwrap_or_else(|e| panic!("stage {n}: {e}"));
        println!("staged {n}");
    }
    let r = apply(&env, &m, true, "offline-t7").expect("apply");
    println!("{r}");
}
