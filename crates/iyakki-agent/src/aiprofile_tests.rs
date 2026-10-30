use super::*;
use std::path::PathBuf;

fn tmp(tag: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let p = PathBuf::from(std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into())).join(format!("iyakki-aip-{}-{n}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn put(root: &Path, rel: &str, text: &str) {
    let f = root.join(rel);
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    std::fs::write(f, text).unwrap();
}

fn mem(root: &Path, gb: u64) {
    put(root, "proc/meminfo", &format!("MemTotal:       {} kB\nMemFree: 1 kB\n", gb * 1024 * 1024));
}

fn card(root: &Path, addr: &str, vendor: &str, device: &str, files: &[(&str, String)]) {
    let d = format!("sys/bus/pci/devices/{addr}");
    put(root, &format!("{d}/class"), "0x030000\n");
    put(root, &format!("{d}/vendor"), &format!("{vendor}\n"));
    put(root, &format!("{d}/device"), &format!("{device}\n"));
    std::fs::create_dir_all(root.join(&d).join("drm/card0")).unwrap();
    for (f, c) in files {
        put(root, &format!("{d}/{f}"), c);
    }
}

const G: u64 = 1 << 30;

fn mac(model: &str, ram: u64) -> AiProfile {
    let r = tmp(&format!("mac-{}", model.replace(',', "_")));
    put(&r, "sys/class/dmi/id/product_name", &format!("{model}\n"));
    mem(&r, ram);
    let p = detect(&r);
    let _ = std::fs::remove_dir_all(&r);
    p
}

fn pc(ram: u64, cards: &[(&str, &str, &str, Vec<(&str, String)>)]) -> AiProfile {
    let r = tmp(&format!("pc{}", std::process::id() as u128 + cards.len() as u128 + ram as u128 * 7));
    put(&r, "sys/class/dmi/id/product_name", "Some PC\n");
    mem(&r, ram);
    for (a, v, d, f) in cards {
        card(&r, a, v, d, f);
    }
    let p = detect(&r);
    let _ = std::fs::remove_dir_all(&r);
    p
}

fn nv(device: &str) -> AiProfile {
    pc(32, &[("0000:01:00.0", "0x10de", device, vec![])])
}

#[test]
fn apple_silicon_models_map_to_generations_and_tiers() {
    let t = |m: &str| apple_gen_of_model(m);
    assert_eq!(t("MacBookAir10,1"), Some((Gen::M1, Some(ChipTier::Base))));
    assert_eq!(t("MacBookPro17,1"), Some((Gen::M1, Some(ChipTier::Base))));
    assert_eq!(t("MacBookPro18,2"), Some((Gen::M1, Some(ChipTier::Max))));
    assert_eq!(t("MacBookPro18,3"), Some((Gen::M1, Some(ChipTier::Pro))));
    assert_eq!(t("Macmini9,1"), Some((Gen::M1, Some(ChipTier::Base))));
    assert_eq!(t("Mac13,2"), Some((Gen::M1, Some(ChipTier::Ultra))));
    assert_eq!(t("Mac14,9"), Some((Gen::M2, Some(ChipTier::Pro))));
    assert_eq!(t("Mac14,14"), Some((Gen::M2, Some(ChipTier::Ultra))));
    assert_eq!(t("Mac15,9"), Some((Gen::M3, Some(ChipTier::Max))));
    assert_eq!(t("Mac16,1").map(|x| x.0), Some(Gen::M4));
    assert_eq!(t("Mac17,2").map(|x| x.0), Some(Gen::M5));
    assert_eq!(t("Mac18,1").map(|x| x.0), Some(Gen::M6));
    assert_eq!(t("Mac19,1").map(|x| x.0), Some(Gen::Unknown), "a newer id is Apple Silicon of unknown generation");
    assert_eq!(t("MacBookPro22,1").map(|x| x.0), Some(Gen::Unknown));
    // Intel Macs and PCs are not Apple Silicon
    for m in ["MacBookPro11,1", "MacBookPro16,1", "MacBookAir9,1", "Macmini8,1", "Macmini7,1", "iMac20,1", "iMacPro1,1", "MacPro7,1", "Mac12,1", "Some PC", "", "Mac", "Mac,", "Mac1,x", "Mac99999999999,1"] {
        assert!(t(m).is_none() || m == "Mac99999999999,1", "{m}");
    }
    assert_eq!(apple_gen_of_text("Apple MacBook Pro (14-inch, M2 Pro, 2023)"), Some((Gen::M2, Some(ChipTier::Pro))));
    assert_eq!(apple_gen_of_text("Apple Mac Studio (M1 Ultra, 2022)"), Some((Gen::M1, Some(ChipTier::Ultra))));
    assert_eq!(apple_gen_of_text("Apple MacBook Air (M1, 2020)"), Some((Gen::M1, Some(ChipTier::Base))));
    assert_eq!(apple_gen_of_text("Raspberry Pi 4 Model B"), None);
    assert_eq!(apple_gen_of_text("Apple "), None);
}

#[test]
fn the_table_of_machines() {
    // (profile, usable GB, tier, line)
    let m1 = mac("MacBookAir10,1", 8);
    assert!(matches!(m1.class, Class::AppleSilicon { gen: Gen::M1, .. }));
    assert!((m1.llm_usable_gb - 16.0 / 3.0).abs() < 0.01);
    assert_eq!(m1.tier, Tier::None);
    assert!(!m1.capable());
    assert_eq!(m1.line(), "Apple M1, 8 GB unified memory: about 5 GB usable for models");

    let m2 = mac("Mac14,9", 32);
    assert!((m2.llm_usable_gb - 64.0 / 3.0).abs() < 0.01);
    assert_eq!(m2.tier, Tier::Large);
    assert_eq!(m2.line(), "Apple M2 Pro, 32 GB unified memory: about 21 GB usable for models");
    assert_eq!(m2.fits_line(), "fits: 3B Q4, 7B Q4, 13B Q4, 32B Q4");

    let m3 = mac("Mac15,9", 64);
    assert_eq!(m3.llm_usable_gb, 48.0, "above 36 GB three quarters");
    assert_eq!(m3.tier, Tier::Xl);
    assert_eq!(m3.line(), "Apple M3 Max, 64 GB unified memory: about 48 GB usable for models");

    // an Asahi device tree names the chip and wins over the model id
    let r = tmp("m4u");
    put(&r, "proc/device-tree/model", "Apple Mac Studio (2025, M4 Ultra)\0");
    mem(&r, 192);
    let m4 = detect(&r);
    assert!(matches!(m4.class, Class::AppleSilicon { gen: Gen::M4, tier: Some(ChipTier::Ultra), .. }));
    assert_eq!(m4.llm_usable_gb, 144.0);
    assert_eq!(m4.tier, Tier::Xl);
    let _ = std::fs::remove_dir_all(&r);

    assert!(matches!(mac("Mac17,2", 24).class, Class::AppleSilicon { gen: Gen::M5, .. }));
    assert!(matches!(mac("Mac18,3", 32).class, Class::AppleSilicon { gen: Gen::M6, .. }));
    let unk = mac("Mac21,4", 48);
    assert!(matches!(unk.class, Class::AppleSilicon { gen: Gen::Unknown, .. }));
    assert_eq!(unk.llm_usable_gb, 36.0);
    assert!(unk.line().starts_with("Apple M-series, 48 GB") && unk.notes.iter().any(|n| n.contains("newer Apple Silicon")));

    let r3060 = nv("0x2503");
    assert!(matches!(&r3060.class, Class::DiscreteGpu { vendor: Vendor::Nvidia, vram_gb: Some(v), .. } if *v == 12.0));
    assert_eq!(r3060.llm_usable_gb, 10.5);
    assert_eq!(r3060.tier, Tier::Medium);
    assert_eq!(r3060.line(), "NVIDIA GeForce RTX 3060, 12 GB: about 10.5 GB usable for models");
    assert_eq!(r3060.fits_line(), "fits: 3B Q4, 7B Q4, 13B Q4");
    assert!(r3060.notes.iter().any(|n| n.contains("not measured")), "{:?}", r3060.notes);

    let r4090 = nv("0x2684");
    assert_eq!((r4090.llm_usable_gb, r4090.tier), (22.5, Tier::Large));

    let gtx = nv("0x1c81");
    assert_eq!(gtx.llm_usable_gb, 0.5);
    assert_eq!(gtx.tier, Tier::None);
    assert!(!gtx.capable() && !gtx.unknown());

    let xtx = pc(64, &[("0000:03:00.0", "0x1002", "0x744c", vec![("mem_info_vram_total", format!("{}\n", 24 * G)), ("mem_info_gtt_total", format!("{}\n", 32 * G))])]);
    assert!(matches!(&xtx.class, Class::DiscreteGpu { vendor: Vendor::Amd, vram_gb: Some(v), .. } if *v == 24.0));
    assert_eq!((xtx.llm_usable_gb, xtx.tier), (22.5, Tier::Large));
    assert!(xtx.line().starts_with("AMD Radeon RX 7900 XT/XTX, 24 GB"), "{}", xtx.line());

    let apu = pc(16, &[("0000:c5:00.0", "0x1002", "0x15bf", vec![("mem_info_vram_total", format!("{}\n", G / 2)), ("mem_info_gtt_total", format!("{}\n", 8 * G))])]);
    assert!(matches!(&apu.class, Class::IntegratedGpu { carve_out_gb, gtt_gb, .. } if *carve_out_gb == 0.5 && *gtt_gb == 8.0));
    assert_eq!(apu.llm_usable_gb, 8.0);
    assert_eq!(apu.tier, Tier::Small);
    assert_eq!(apu.line(), "AMD Radeon 780M (Phoenix) (shared): up to 8 GB usable for models");
    // the shared pool never takes the system's own share: 8 GB RAM leaves 4 GB
    let apu8 = pc(8, &[("0000:c5:00.0", "0x1002", "0x15bf", vec![("mem_info_vram_total", format!("{}\n", G / 2)), ("mem_info_gtt_total", format!("{}\n", 8 * G))])]);
    assert_eq!(apu8.llm_usable_gb, 4.0);
    assert!(!apu8.capable());

    let arc = pc(32, &[("0000:03:00.0", "0x8086", "0x56a0", vec![])]);
    // no size from the driver: unknown, not hidden
    assert!(arc.unknown() || matches!(arc.class, Class::DiscreteGpu { .. }));
    let r = tmp("arc");
    mem(&r, 32);
    card(&r, "0000:03:00.0", "0x8086", "0x56a0", &[]);
    put(&r, "sys/bus/pci/devices/0000:03:00.0/drm/card0/lmem_total_bytes", &format!("{}\n", 16 * G));
    let arc = detect(&r);
    assert_eq!((arc.llm_usable_gb, arc.tier), (14.5, Tier::Medium));
    assert!(arc.line().starts_with("Intel Arc A770, 16 GB"));
    let _ = std::fs::remove_dir_all(&r);

    // Intel integrated graphics are not used for models
    let uhd = pc(16, &[("0000:00:02.0", "0x8086", "0x3e92", vec![])]);
    assert!(matches!(uhd.class, Class::CpuOnly { .. }));
    assert_eq!(uhd.line(), "Intel UHD Graphics 630 (not used for models): CPU only, 16 GB RAM");

    let c4 = pc(4, &[]);
    assert!(matches!(c4.class, Class::CpuOnly { .. }));
    assert_eq!((c4.llm_usable_gb, c4.tier), (0.0, Tier::None));
    assert_eq!(c4.line(), "No GPU: CPU only, 4 GB RAM");
    let c64 = pc(64, &[]);
    assert_eq!((c64.llm_usable_gb, c64.tier, c64.capable()), (60.0, Tier::None, false), "plenty of RAM is still the CPU: slow");
    assert!(c64.summary().contains("CPU and be slow"));
    let c155 = {
        let r = tmp("c155");
        put(&r, "proc/meminfo", "MemTotal:       16253952 kB\n");
        let p = detect(&r);
        let _ = std::fs::remove_dir_all(&r);
        p
    };
    assert_eq!(c155.line(), "No GPU: CPU only, 15.5 GB RAM");

    let two = pc(32, &[("0000:00:02.0", "0x1002", "0x15bf", vec![("mem_info_vram_total", format!("{}\n", G / 2)), ("mem_info_gtt_total", format!("{}\n", 8 * G))]), ("0000:01:00.0", "0x10de", "0x2503", vec![])]);
    assert_eq!(two.gpus.len(), 2);
    assert!(matches!(&two.class, Class::DiscreteGpu { vendor: Vendor::Nvidia, .. }));
    assert_eq!(two.llm_usable_gb, 10.5, "the best card, not the sum");
    assert!(two.notes.iter().any(|n| n.contains("2 graphics devices")));
}

#[test]
fn a_gpu_of_unknown_size_is_honest_and_hides_nothing() {
    let p = nv("0x1234");
    assert!(p.unknown() && !p.capable());
    assert!(p.line().contains("memory size unknown"));
    for i in Item::ALL {
        assert!(rule(&p, i).shown(), "{i:?} stays");
    }
    // a big resizable BAR is believed (4 GB or more), a 256 MB window is not
    let r = tmp("bar");
    mem(&r, 32);
    card(&r, "0000:01:00.0", "0x10de", "0x1234", &[("resource", "0x00000000f6000000 0x00000000f6ffffff 0x0000000000040200\n0x0000004000000000 0x00000043ffffffff 0x000000000014220c\n".into())]);
    let big = detect(&r);
    assert_eq!(big.llm_usable_gb, 14.5, "16 GB window");
    assert!(big.notes.iter().any(|n| n.contains("PCI window")));
    card(&r, "0000:01:00.0", "0x10de", "0x1234", &[("resource", "0x00000000e0000000 0x00000000efffffff 0x000000000014220c\n".into())]);
    assert!(detect(&r).unknown());
    let _ = std::fs::remove_dir_all(&r);
}

#[test]
fn thresholds_tiers_and_reserves_are_the_documented_numbers() {
    assert_eq!((MIN_LLM_GB, DISCRETE_RESERVE_GB, SYSTEM_RESERVE_GB, CONTEXT_GB), (6.0, 1.5, 4.0, 1.0));
    for (gb, t) in [(0.0, Tier::None), (5.99, Tier::None), (6.0, Tier::Small), (9.99, Tier::Small), (10.0, Tier::Medium), (20.99, Tier::Medium), (21.0, Tier::Large), (42.99, Tier::Large), (43.0, Tier::Xl), (500.0, Tier::Xl)] {
        assert_eq!(Tier::of(gb), t, "{gb} GB");
    }
    // the card sizes people buy land where the documentation says
    for (vram, t) in [(2.0, Tier::None), (4.0, Tier::None), (6.0, Tier::None), (8.0, Tier::Small), (12.0, Tier::Medium), (16.0, Tier::Medium), (24.0, Tier::Large), (48.0, Tier::Xl)] {
        assert_eq!(Tier::of(vram - DISCRETE_RESERVE_GB), t, "{vram} GB card");
    }
    assert!(Tier::None < Tier::Small && Tier::Large < Tier::Xl);
    assert_eq!(Tier::Medium.words(), "runs 7B and 13B models comfortably");
    // an 8 GB card has 6.5 usable: capable; a 6 GB card has 4.5: not
    assert!(nv("0x2487").capable());
    assert!(!nv("0x1c03").capable());
    // Apple: two thirds up to 36 GB (inclusive), three quarters above
    assert_eq!(mac("Mac14,2", 36).llm_usable_gb, 24.0);
    assert_eq!(mac("Mac14,2", 48).llm_usable_gb, 36.0);
    // unreadable memory: zero, never a panic
    assert_eq!(ram_gb(Path::new("/nonexistent-iyakki")), 0.0);
}

fn profiles() -> Vec<(&'static str, AiProfile)> {
    vec![("m1-8", mac("MacBookAir10,1", 8)), ("m2pro-32", mac("Mac14,9", 32)), ("rtx3060", nv("0x2503")), ("gtx1050", nv("0x1c81")), ("cpu4", pc(4, &[])), ("cpu64", pc(64, &[])), ("unknown", nv("0x1234"))]
}

#[test]
fn the_show_hide_table_per_profile_for_every_item() {
    use Item::*;
    for (name, p) in profiles() {
        for i in Item::ALL {
            let v = rule(&p, i);
            match (name, i) {
                ("m2pro-32" | "rtx3060", Lemonade | Ollama) => assert_eq!(v, Vis::Show, "{name} {i:?}"),
                ("gtx1050" | "cpu4" | "cpu64", Lemonade | Ollama | GpuVulkan) => assert!(matches!(v, Vis::Hidden(_)), "{name} {i:?}"),
                ("m1-8", Lemonade | Ollama) => assert!(matches!(v, Vis::NotRecommended(_)), "{name} {i:?}"),
                ("m1-8" | "m2pro-32", GpuVulkan | GpuNvidia | MacHost) => assert!(matches!(v, Vis::Hidden(_)), "{name} {i:?}"),
                ("rtx3060", GpuVulkan) => assert!(matches!(v, Vis::Recommended(_))),
                ("rtx3060", GpuNvidia) => assert_eq!(v, Vis::Show),
                ("rtx3060", MacHost) => assert!(matches!(v, Vis::NotRecommended(_))),
                ("gtx1050", GpuNvidia) => assert!(matches!(v, Vis::Hidden(_))),
                ("cpu4" | "cpu64" | "gtx1050", MacHost) => assert!(matches!(v, Vis::Recommended(_)), "{name}"),
                ("cpu4" | "cpu64", GpuNvidia) => assert!(matches!(v, Vis::Hidden(_))),
                ("cpu4", Elasticsearch) => assert!(matches!(v, Vis::NotRecommended(_))),
                ("unknown", MacHost) => assert_eq!(v, Vis::Show),
                // CPU-friendly and unrelated tools are always shown
                (_, Node | Python | K3s | DuckDb | Registry | ClaudeCode | Copilot | Kokoro | Ffmpeg | MacFirmware | MacCamera | LinuxApps | LinuxWindows | FwQualcommBt | FwRealtekNet | FwBroadcomNet | FwAtheros9170) => assert_eq!(v, Vis::Show, "{name} {i:?}"),
                _ => {}
            }
        }
    }
    // the hard rules: core and CPU tools never hide anywhere
    for (name, p) in profiles() {
        for i in [Kokoro, K3s, Node, Python, ClaudeCode] {
            assert!(rule(&p, i).shown(), "{name} {i:?}");
        }
    }
}

#[test]
fn the_reasons_are_plain_words_and_show_anyway_brings_every_tool_back() {
    let cpu = pc(8, &[]);
    let v = rule(&cpu, Item::Lemonade);
    assert_eq!(v, Vis::Hidden("this machine has no GPU with 6 GB or more; local models would run on the CPU and be very slow".into()));
    assert_eq!(hidden_text(v.reason().unwrap()), "Hidden because this machine has no GPU with 6 GB or more; local models would run on the CPU and be very slow.");
    let gtx = nv("0x1c81");
    let r = rule(&gtx, Item::Ollama);
    assert!(r.reason().unwrap().contains("NVIDIA GeForce GTX 1050") && r.reason().unwrap().contains("under 6 GB"), "{r:?}");
    // Show anyway: shown, not recommended, same reason
    for p in [&cpu, &gtx] {
        for i in Item::ALL {
            let off = visibility(p, i, false);
            let on = visibility(p, i, true);
            assert!(on.shown(), "{i:?}");
            if let Vis::Hidden(r) = off {
                assert_eq!(on, Vis::NotRecommended(r));
            } else {
                assert_eq!(on, off);
            }
        }
    }
    assert!(hidden_items(&cpu).iter().any(|(i, _)| *i == Item::Lemonade));
    assert!(!hidden_items(&cpu).iter().any(|(i, _)| *i == Item::Kokoro));
    assert!(rule(&mac("Mac14,9", 32), Item::MacHost).reason().unwrap().contains("this machine is the Mac"));
}

#[test]
fn the_summary_says_what_it_means_and_a_paired_mac_is_mentioned() {
    assert_eq!(mac("Mac14,9", 32).summary(), "Apple M2 Pro, 32 GB unified memory: about 21 GB usable for models; runs models up to 32B comfortably");
    let r = tmp("hosts");
    mem(&r, 8);
    let p = detect_with_hosts(&r, 2);
    assert_eq!(p.summary(), "No GPU: CPU only, 8 GB RAM; local models would run on the CPU and be slow. 2 Mac model hosts set up");
    assert_eq!(detect_with_hosts(&r, 1).summary(), "No GPU: CPU only, 8 GB RAM; local models would run on the CPU and be slow. 1 Mac model host set up");
    let _ = std::fs::remove_dir_all(&r);
    assert_eq!(nv("0x2503").fits(), vec!["3B", "7B", "13B"]);
    assert_eq!(pc(4, &[]).fits_line(), "fits: no model of useful size");
}

#[test]
fn garbage_files_never_panic() {
    let r = tmp("garbage");
    let junk = ["", "\0\0\0", "not a number", "-1", "99999999999999999999999999", "0xZZ", "\u{ffff}\u{1f600}", "Apple", "Apple M", "Apple M9 Pro Max"];
    for (n, j) in junk.iter().enumerate() {
        for f in ["proc/meminfo", "sys/class/dmi/id/product_name", "proc/device-tree/model", "sys/bus/pci/devices/0000:01:00.0/mem_info_vram_total", "sys/bus/pci/devices/0000:01:00.0/mem_info_gtt_total", "sys/bus/pci/devices/0000:01:00.0/resource", "sys/bus/pci/devices/0000:01:00.0/drm/card0/lmem_total_bytes"] {
            put(&r, f, j);
        }
        for (v, d) in [("0x1002", "0x15bf"), ("0x1002", "0x744c"), ("0x10de", "0x2503"), ("0x8086", "0x56a0"), ("0xffff", "zz")] {
            card(&r, "0000:01:00.0", v, d, &[]);
            let p = detect(&r);
            let _ = (p.line(), p.summary(), p.fits_line(), hidden_items(&p));
            for i in Item::ALL {
                let _ = visibility(&p, i, n % 2 == 0);
            }
        }
    }
    assert_eq!(detect(Path::new("/nonexistent-iyakki")).line(), "No GPU: CPU only, 0 GB RAM");
    let _ = std::fs::remove_dir_all(&r);
}
