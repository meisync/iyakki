//! Kernel-module and driver-pack checks as typed local calls (F-ARCH-07): `iyakki store` (the publish commands `inspect-module` and
//! `driver-check`), the driver store's installer and the module-pack card read a `.ko` file's own record of the kernel it was built
//! for, decode a pack's base64 payload and parse a pack through this module, and never name the drivers crate. Pure functions on
//! bytes and text: nothing here opens a device.

pub use iyakki_drivers::modinfo::Modinfo;
pub use iyakki_drivers::Pack;

/// A module's own record: name, vermagic, licence, dependencies (no check against a kernel).
pub fn inspect(ko: &[u8]) -> Result<Modinfo, String> {
    iyakki_drivers::modinfo::parse(ko)
}

/// The module checked against the running kernel's release and, when given, the full vermagic of one stock module (which also catches
/// a different build configuration).
pub fn check(ko: &[u8], running_release: &str, reference_vermagic: Option<&str>) -> Result<Modinfo, String> {
    iyakki_drivers::modinfo::check(ko, running_release, reference_vermagic)
}

/// The vermagic of a reference module file's bytes.
pub fn vermagic_of(ko: &[u8]) -> Result<String, String> {
    inspect(ko).map(|m| m.vermagic)
}

/// A pack's base64 payload as bytes; `None` when it is not valid base64.
pub fn decode_payload(b64: &str) -> Option<Vec<u8>> {
    iyakki_drivers::b64::decode(b64)
}

/// A pack from its JSON text.
pub fn parse_pack(json: &str) -> Result<Pack, String> {
    Pack::parse(json)
}

/// SHA-256 as 64 lowercase hex digits (the pack's own file check).
pub fn sha256_hex(bytes: &[u8]) -> String {
    iyakki_drivers::pack::sha256_hex(bytes)
}

pub use iyakki_drivers::run::Runner;

/// The program runner a pack install uses: the real one on a real data folder, and one that runs nothing on a made-up one.
pub fn runner(real: bool) -> Box<dyn Runner> {
    if real { Box::new(iyakki_drivers::run::System) } else { Box::new(iyakki_drivers::run::Dry) }
}

/// `depmod` for the optional driver pack's install: the module index of a kernel release, rebuilt with the system's own `depmod`.
pub struct LocalDepmod;

impl iyakki_drivers::modpack::Depmod for LocalDepmod {
    fn run(&self, _root: &std::path::Path, release: &str) -> Result<(), String> {
        if release.is_empty() || release.contains('/') || release.starts_with('-') {
            return Err(format!("`{release}` is not a kernel release"));
        }
        crate::starters::depmod(release)
    }
}

/// Is this computer's product name a model with the Cirrus CS8409 sound codec (the Apple laptops whose sound card needs the quiet fix)?
pub fn is_cs8409_model(product: &str) -> bool {
    iyakki_drivers::cs8409::MODELS.contains(&product.trim())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_depmod_of_the_pack_install_refuses_a_release_that_is_not_one_before_any_program_starts() {
        use iyakki_drivers::modpack::Depmod;
        for bad in ["", "../x", "-a", "6.1/x"] {
            assert!(super::LocalDepmod.run(std::path::Path::new("/nonexistent"), bad).unwrap_err().contains("not a kernel release"), "{bad}");
        }
    }

    use super::*;
    use iyakki_drivers::modinfo::{synthetic_module, synthetic_module_with};

    #[test]
    fn a_module_says_its_name_licence_and_dependencies_and_a_non_free_licence_is_named() {
        let ko = synthetic_module_with("wl", "6.12.112 SMP preempt mod_unload ", "MIXED/Proprietary", "cfg80211");
        let m = inspect(&ko).unwrap();
        assert_eq!((m.name.as_str(), m.release()), ("wl", "6.12.112"));
        assert!(!m.gpl_compatible());
        assert_eq!(m.depends, ["cfg80211"]);
        assert!(inspect(b"not an elf").is_err());
    }

    #[test]
    fn a_module_for_another_kernel_or_other_build_settings_is_refused_in_words() {
        let ko = synthetic_module("a", "6.12.112 SMP preempt mod_unload ", "GPL");
        assert!(check(&ko, "6.12.112", None).is_ok());
        assert!(check(&ko, "6.12.113", None).unwrap_err().contains("built for kernel 6.12.112"));
        let stock = vermagic_of(&synthetic_module("s", "6.12.112 SMP mod_unload ", "GPL")).unwrap();
        assert!(check(&ko, "6.12.112", Some(&stock)).unwrap_err().contains("build settings"));
        assert!(check(&ko, "6.12.112", Some("")).is_ok(), "an empty reference is no reference");
    }

    #[test]
    fn a_payload_round_trips_through_base64_and_the_sha_is_hex() {
        let bytes = b"firmware bytes \x00\x01\x02".to_vec();
        let text = iyakki_drivers::b64::encode(&bytes);
        assert_eq!(decode_payload(&text), Some(bytes.clone()));
        assert_eq!(decode_payload("!!! not base64 !!!"), None);
        let h = sha256_hex(&bytes);
        assert_eq!(h.len(), 64);
        assert!(h.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn a_pack_text_that_is_not_a_pack_is_refused() {
        assert!(parse_pack("{}").is_err() || parse_pack("{}").unwrap().modules.is_empty());
        assert!(parse_pack("not json").is_err());
    }

    #[test]
    fn the_dry_runner_runs_nothing_and_succeeds_and_the_codec_models_are_told_by_name() {
        assert!(runner(false).run("definitely-not-a-program", &["x"]).is_ok());
        let first = iyakki_drivers::cs8409::MODELS[0];
        assert!(is_cs8409_model(first) && is_cs8409_model(&format!("  {first}\n")));
        assert!(!is_cs8409_model("ThinkPad X1") && !is_cs8409_model(""));
    }
}
