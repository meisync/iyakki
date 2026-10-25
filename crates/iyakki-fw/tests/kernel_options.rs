//! The firewall needs these kernel options (docs/FIREWALL.md); the kernel fragments of both architectures must ask for them, so
//! a kernel without nf_tables is noticed in review, not on a device. (`dist/buildroot/verify-kernel-config.sh` then proves the
//! fragment survives kconfig.) Netfilter netlink, the nflog logger, conntrack and the counter come from the kernel's defaults and
//! from nf_tables itself, and were checked in the built kernels' `.config`.

use std::path::Path;

fn fragment(rel: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

#[test]
fn both_kernel_fragments_ask_for_nf_tables_and_the_parts_the_firewall_uses() {
    for f in ["dist/buildroot/board/iyakki/linux.fragment", "dist/buildroot/board/iyakki-aarch64/linux.fragment"] {
        let t = fragment(f);
        for want in ["CONFIG_NF_TABLES=", "CONFIG_NF_TABLES_INET=y", "CONFIG_NFT_CT=", "CONFIG_NFT_LOG=", "CONFIG_NFT_LIMIT="] {
            let line = t.lines().find(|l| l.starts_with(want)).unwrap_or_else(|| panic!("{f} lacks {want}"));
            assert!(line.ends_with("=y") || line.ends_with("=m"), "{f}: {line}");
        }
        assert!(!t.contains("# CONFIG_NF_TABLES is not set"));
    }
}

#[test]
fn the_modules_the_backend_loads_are_the_ones_the_fragment_builds() {
    // nf_tables (NF_TABLES=m) and nft_ct, nft_limit, nft_log are modules; the rest of what the ruleset uses is inside nf_tables.ko
    assert_eq!(iyakki_fw::backend::MODULES, ["nf_tables", "nft_ct", "nft_limit", "nft_log"]);
}
