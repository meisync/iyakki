use super::*;
use crate::archive::tests::tmp;
use crate::fwextra::{TAG, COMMIT};
use iyakki_drivers::pack::sha256_hex;

fn manifest_for(files: &[(&str, &str, &str)]) -> Manifest {
    let mut t = format!("tag {TAG}\ncommit {COMMIT}\nbase https://git.kernel.org/pub/scm/linux/kernel/git/firmware/linux-firmware.git/plain\nsnapshot 1 {} https://cdn.kernel.org/x.tar.gz\n", "ab".repeat(32));
    t.push_str(&format!("doc 5 {} WHENCE\n", sha256_hex(b"whenc")));
    t.push_str(&format!("doc 7 {} LICENSES/LICENCE.iwlwifi_firmware\n", sha256_hex(b"licence")));
    for (p, b, tok) in files {
        t.push_str(&format!("file {} {} {tok} {p}\n", b.len(), sha256_hex(b.as_bytes())));
    }
    fwextra::parse_manifest(&t).unwrap()
}

fn serve(url: &str) -> Vec<u8> {
    match url.split("/plain/").nth(1).unwrap().split('?').next().unwrap() {
        "WHENCE" => b"whenc".to_vec(),
        "LICENSES/LICENCE.iwlwifi_firmware" => b"licence".to_vec(),
        "intel/iwlwifi/iwlwifi-cc-a0-77.ucode" => b"ucode77".to_vec(),
        other => panic!("fetched something that was not asked for: {other}"),
    }
}

#[test]
fn the_other_computer_fetches_only_the_named_files_and_checks_each() {
    let m = manifest_for(&[("intel/iwlwifi/iwlwifi-cc-a0-77.ucode", "ucode77", "LICENCE.iwlwifi_firmware"), ("mediatek/other.bin", "other", "LICENCE.mediatek")]);
    let plan = plan_for_names(&["intel/iwlwifi/iwlwifi-cc-a0-77.ucode".to_string(), "../../etc/passwd".to_string()], &m);
    let out = tmp("fwx-cli-fetch");
    let mut asked: Vec<String> = Vec::new();
    let s = fetch_to_folder(
        &mut |url, _w, dest| {
            asked.push(url.to_string());
            std::fs::write(dest, serve(url)).map_err(|e| e.to_string())
        },
        &m,
        &plan,
        &[],
        &out,
    )
    .unwrap();
    assert_eq!(s.files, 3, "the file, its licence text and WHENCE: {asked:?}");
    assert!(asked.iter().all(|u| u.starts_with("https://git.kernel.org/") && u.ends_with(&format!("?id={COMMIT}"))));
    assert!(out.join("firmware-extra/intel/iwlwifi/iwlwifi-cc-a0-77.ucode").is_file() && out.join("firmware-extra/firmware-extra.json").is_file());
    assert!(!asked.iter().any(|u| u.contains("mediatek") || u.contains("passwd")), "a traversal name is dropped, other files are not fetched");
    // the folder the helper made is the folder the device reads
    assert!(crate::fwextra::find_in_folder(&out, "intel/iwlwifi/iwlwifi-cc-a0-77.ucode").is_some());
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn a_corrupt_download_on_the_other_computer_is_deleted_not_pushed() {
    let m = manifest_for(&[("intel/iwlwifi/iwlwifi-cc-a0-77.ucode", "ucode77", "LICENCE.iwlwifi_firmware")]);
    let plan = plan_for_names(&["intel/iwlwifi/iwlwifi-cc-a0-77.ucode".to_string()], &m);
    let out = tmp("fwx-cli-bad");
    let e = fetch_to_folder(&mut |url, _w, dest| std::fs::write(dest, if url.contains("ucode") { b"ucode78".to_vec() } else { serve(url) }).map_err(|e| e.to_string()), &m, &plan, &[], &out).unwrap_err();
    assert!(e.contains("does not match") && !out.join("firmware-extra/intel/iwlwifi/iwlwifi-cc-a0-77.ucode").exists(), "{e}");
    let empty = plan_for_names(&["nope.bin".to_string()], &m);
    assert!(fetch_to_folder(&mut |_, _, _| Ok(()), &m, &empty, &[], &out).unwrap_err().contains("nothing to fetch"));
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn the_share_preview_is_text_only() {
    let v = share_preview("thunderbolt0", "wlp3s0").unwrap();
    assert!(v.iter().any(|l| l.contains("masquerade")));
    assert!(share_preview("a b", "x").is_err());
}

/// `dist/scripts/firmware-notes/fetch.sh --dry-run` and the item must choose the same files and licence texts for the same names.
#[test]
fn the_developer_script_and_the_item_choose_the_same_files() {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../dist/scripts/firmware-notes/fetch.sh");
    let m = manifest();
    let names = ["iwlwifi-cc-a0-77.ucode", "iwlwifi-cc-a0-76.ucode", "rtw89/rtw8852a_fw.bin", "intel/ibt-0041-0041.sfi", "i915/tgl_guc_70.bin", "brcm/brcmfmac4350c2-pcie.bin", "nvidia/ga102/gsp/booter_load-570.144.bin", "not/in/the-release.bin", "../../etc/passwd"];
    let out = match std::process::Command::new("bash").arg(&script).args(["--dry-run", "--names", &names.join(",")]).output() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("no bash here ({e}): the script comparison is skipped");
            return;
        }
    };
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut from_script: Vec<String> = text.lines().map(str::to_string).collect();
    from_script.sort();
    let plan = plan_for_names(&names.iter().map(|s| s.to_string()).collect::<Vec<_>>(), m);
    let sel = plan.select(&[]);
    let mut from_rust: Vec<String> = sel.unique().keys().map(|p| format!("file {p}")).chain(sel.docs(m).keys().map(|p| format!("doc {p}"))).collect();
    from_rust.sort();
    assert_eq!(from_script, from_rust, "the script and fwextra.rs disagree about the file list");
    assert!(from_rust.iter().any(|l| l == "file intel/iwlwifi/iwlwifi-cc-a0-77.ucode") && !from_rust.iter().any(|l| l.contains("nvidia") || l.contains("passwd") || l.contains("-76")));
    // with NVIDIA included they still agree
    let with = std::process::Command::new("bash").arg(&script).args(["--dry-run", "--with-nvidia", "--names", &names.join(",")]).output().unwrap();
    let mut a: Vec<String> = String::from_utf8_lossy(&with.stdout).lines().map(str::to_string).collect();
    a.sort();
    let sel = plan.select(&["nvidia".to_string()]);
    let mut b: Vec<String> = sel.unique().keys().map(|p| format!("file {p}")).chain(sel.docs(m).keys().map(|p| format!("doc {p}"))).collect();
    b.sort();
    assert_eq!(a, b);
    assert!(a.iter().any(|l| l == "doc LICENSES/LICENCE.nvidia"));
}
