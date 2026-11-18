use super::*;

/// A tar of (name, kind, content-or-link, mode), with a correct checksum, like the one a sandbox makes.
fn tar(entries: &[(&str, u8, &str, u64)]) -> Vec<u8> {
    let mut t = Vec::new();
    for (name, kind, body, mode) in entries {
        let (size, link) = if *kind == b'2' || *kind == b'1' { (0, *body) } else { (body.len(), "") };
        let mut h = [0u8; 512];
        h[..name.len().min(100)].copy_from_slice(&name.as_bytes()[..name.len().min(100)]);
        h[100..108].copy_from_slice(format!("{mode:07o}\0").as_bytes());
        h[108..116].copy_from_slice(b"0001750\0");
        h[116..124].copy_from_slice(b"0001750\0");
        h[124..136].copy_from_slice(format!("{size:011o}\0").as_bytes());
        h[136..148].copy_from_slice(b"14000000000\0");
        h[156] = *kind;
        h[157..157 + link.len()].copy_from_slice(link.as_bytes());
        h[257..263].copy_from_slice(b"ustar\0");
        h[148..156].copy_from_slice(b"        ");
        let sum: u32 = h.iter().map(|b| *b as u32).sum();
        h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        t.extend_from_slice(&h);
        if size > 0 {
            t.extend_from_slice(body.as_bytes());
            t.extend(std::iter::repeat_n(0u8, size.div_ceil(512) * 512 - size));
        }
    }
    t.extend(std::iter::repeat_n(0u8, 1024));
    t
}

fn names(layer: &[u8]) -> Vec<(String, u8, String)> {
    let mut at = 0;
    let mut v = vec![];
    while at + 512 <= layer.len() && layer[at..at + 512].iter().any(|b| *b != 0) {
        let h = &layer[at..at + 512];
        let size = u64::from_str_radix(field_str(&h[124..136]).trim(), 8).unwrap() as usize;
        let name = field_str(&h[..100]);
        let prefix = field_str(&h[345..500]);
        v.push((if prefix.is_empty() { name } else { format!("{prefix}/{name}") }, h[156], u32::from_str_radix(field_str(&h[100..108]).trim(), 8).map(|m| format!("{m:o}")).unwrap()));
        at += 512 + size.div_ceil(512) * 512;
    }
    v
}

#[test]
fn a_root_becomes_one_clean_layer_without_devices_setid_bits_owners_or_times() {
    let t = tar(&[
        ("./", b'5', "", 0o755),
        ("./bin/", b'5', "", 0o755),
        ("./bin/app", b'0', "program", 0o4755),
        ("./bin/sh", b'2', "busybox", 0o777),
        ("./dev/null", b'3', "", 0o666),
        ("./tmp/pipe", b'6', "", 0o644),
        ("./etc/conf", b'0', "x=1", 0o640),
        ("./bin/app2", b'1', "./bin/app", 0o755),
    ]);
    let (layer, rep) = layer_from_root_tar(&t, 1 << 20).unwrap();
    assert_eq!((rep.files, rep.dirs, rep.links, rep.bytes, rep.setid_cleared), (2, 1, 2, 10, 1), "{rep:?}");
    assert_eq!(rep.dropped, ["dev/null", "tmp/pipe"], "devices and fifos are left to the container");
    let n = names(&layer);
    assert_eq!(n.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["bin/", "bin/app", "bin/sh", "etc/conf", "bin/app2"]);
    assert_eq!(n[1].2, "755", "the set-user-id bit is gone");
    assert_eq!(n[3].2, "640");
    // owners and times are zero, so the same root is always the same layer
    assert_eq!(&layer[108..116], b"0000000\0");
    assert_eq!(&layer[136..148], b"00000000000\0");
    assert_eq!(layer_from_root_tar(&t, 1 << 20).unwrap().0, layer);
}

#[test]
fn unsafe_names_a_damaged_tar_an_empty_one_and_an_oversized_root_are_refused() {
    let e = layer_from_root_tar(&tar(&[("./../etc/passwd", b'0', "x", 0o644)]), 1 << 20).unwrap_err();
    assert!(e.contains("..") && e.contains("refused"), "{e}");
    let mut bad = tar(&[("a", b'0', "x", 0o644)]);
    bad[10] ^= 1;
    assert!(layer_from_root_tar(&bad, 1 << 20).unwrap_err().contains("checksum"));
    assert!(layer_from_root_tar(&tar(&[("a", b'0', "xxxxxxxxxxxx", 0o644)]), 5).unwrap_err().contains("bigger than 5 bytes"));
    assert!(layer_from_root_tar(&tar(&[]), 100).unwrap_err().contains("no files"));
    assert!(layer_from_root_tar(&tar(&[("a", b'0', "x", 0o644)])[..512], 100).unwrap_err().contains("cut off"));
    assert!(layer_from_root_tar(&tar(&[("x", b'1', "../outside", 0o644)]), 100).unwrap_err().contains(".."));
}

#[test]
fn a_gnu_long_name_and_a_pax_path_are_carried_into_the_ustar_prefix() {
    let long = format!("usr/share/{}/{}/file.txt", "d".repeat(60), "e".repeat(60));
    assert!(long.len() > 100 && long.len() < 200);
    // GNU style: a `L` entry holds the name
    let mut t = tar(&[("././@LongLink", b'L', &format!("{long}\0"), 0o644), ("short", b'0', "data", 0o644)]);
    let (layer, rep) = layer_from_root_tar(&t, 1000).unwrap();
    assert_eq!(rep.files, 1);
    assert_eq!(names(&layer)[0].0, long);
    // pax style
    let body = format!(" path={long}\n");
    let mut n = body.len() + 1;
    while n.to_string().len() + body.len() != n {
        n = n.to_string().len() + body.len();
    }
    let rec = format!("{n}{body}");
    t = tar(&[("pax", b'x', &rec, 0o644), ("short", b'0', "data", 0o644)]);
    let (layer, _) = layer_from_root_tar(&t, 1000).unwrap();
    assert_eq!(names(&layer)[0].0, long);
    // a path that cannot be split into prefix and name is refused with its name
    let unsplittable = "z".repeat(150);
    let e = layer_from_root_tar(&tar(&[("././@LongLink", b'L', &format!("{unsplittable}\0"), 0o644), ("s", b'0', "d", 0o644)]), 1000).unwrap_err();
    assert!(e.contains("too long"), "{e}");
}

#[test]
fn the_image_names_its_user_entry_point_port_and_layer_and_refuses_what_it_cannot_run() {
    let t = tar(&[("./app", b'0', "program", 0o755)]);
    let spec = RootImageSpec { arch: "amd64", user: "10001:10001", entrypoint: vec!["/app".into()], cmd: vec!["serve".into()], env: vec![("MODE".into(), "demo".into())], workdir: "/", port: Some(8080) };
    let (img, rep) = build_image_from_root(&t, &spec, 1 << 20).unwrap();
    assert_eq!(rep.files, 1);
    let cfg: serde_json::Value = serde_json::from_slice(&img.config).unwrap();
    assert_eq!((cfg["config"]["User"].as_str(), cfg["config"]["Entrypoint"][0].as_str(), cfg["config"]["Cmd"][0].as_str(), cfg["config"]["Env"][0].as_str()), (Some("10001:10001"), Some("/app"), Some("serve"), Some("MODE=demo")));
    assert!(cfg["config"]["ExposedPorts"]["8080/tcp"].is_object());
    assert_eq!(cfg["rootfs"]["diff_ids"][0], img.layer_digest.as_str());
    let m: serde_json::Value = serde_json::from_slice(&img.manifest).unwrap();
    assert_eq!(m["layers"][0]["digest"], img.layer_digest.as_str());
    assert_eq!(m["config"]["digest"], img.config_digest.as_str());
    // it saves as an OCI layout like the one-file image
    assert!(crate::ocipush::layout_tar(&img, "my-notes:1").is_ok());
    for bad in [RootImageSpec { arch: "riscv", ..spec.clone() }, RootImageSpec { entrypoint: vec![], cmd: vec![], ..spec.clone() }, RootImageSpec { user: "root", ..spec.clone() }] {
        assert!(build_image_from_root(&t, &bad, 1 << 20).is_err());
    }
}
