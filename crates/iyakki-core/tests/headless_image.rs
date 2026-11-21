//! The headless web service image (variant "headless", F-WEBSVC-15). No image is built here: the recipe that wires the variant is checked as
//! files (the way pc_image.rs checks the PC variant): the fragment adds no desktop package, build.sh builds the program without the shell and
//! refuses what the variant cannot have, post-build installs the cloud service file, `changed.sh` composes the same defconfig, and the service
//! file the image carries is a valid, complete answer to "what do I serve" for an init that is started with no arguments.

use std::path::{Path, PathBuf};

fn dist() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(dist().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// The settings (not the comments) of a fragment.
fn settings(rel: &str) -> Vec<String> {
    read(rel).lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(String::from).collect()
}

#[test]
fn the_fragment_adds_no_desktop_graphics_browser_or_sound_package_and_the_base_has_none_to_start_with() {
    let frag = settings("buildroot/configs/iyakki_headless.fragment");
    for l in &frag {
        let up = l.to_ascii_uppercase();
        for desktop in ["MESA", "XORG", "WAYLAND", "WEBKIT", "CHROMIUM", "FIREFOX", "FFMPEG", "PULSEAUDIO", "PIPEWIRE", "SDL", "QT5", "QT6", "GTK", "WPEWEBKIT", "COG", "LIBDRM"] {
            assert!(!up.contains(desktop), "the headless fragment must not add {desktop}: {l}");
        }
    }
    // the base the variant starts from has no graphics stack or browser either (those are the web and mac variants)
    for l in settings("buildroot/configs/iyakki_x86_64_defconfig") {
        let up = l.to_ascii_uppercase();
        for desktop in ["BR2_PACKAGE_MESA3D=Y", "BR2_PACKAGE_XORG7=Y", "BR2_PACKAGE_WEBKITGTK=Y", "BR2_PACKAGE_WPEWEBKIT=Y", "BR2_PACKAGE_CHROMIUM=Y", "BR2_PACKAGE_FFMPEG=Y", "BR2_PACKAGE_PULSEAUDIO=Y", "BR2_PACKAGE_PIPEWIRE=Y"] {
            assert!(!up.contains(desktop), "the base defconfig has {desktop}");
        }
    }
}

#[test]
fn build_sh_builds_the_program_without_the_shell_and_refuses_what_a_headless_image_cannot_have() {
    let b = read("buildroot/build.sh");
    let at = b.find("if [ \"$VARIANT\" = \"headless\" ]; then\n  [ \"$ARCH\"").expect("the headless variant block");
    let block = &b[at..b[at..].find("\nfi\n").map(|e| at + e).unwrap()];
    assert!(block.contains("x86_64 only") && block.contains("exit 2"), "{block}");
    assert!(block.contains("WITH_FFMPEG") && block.contains("exit 2"), "the media player is refused");
    assert!(block.contains("iyakki_headless.fragment") && block.contains("IYAKKI_HEADLESS=1"));
    // the program is made with no default features, so no shell, screen or browser engine is in it, and into its own target folder
    assert!(b.contains("--no-default-features --features headless") && b.contains("CARGO_TARGET_DIR=\"$ROOT/target/headless\""));
    assert!(b.contains("IYAKKI_BIN=\"$ROOT/target/headless/$RUST_TARGET/$PROFILE/iyakki\""));
    // the variant is one of the words the script takes
    assert!(b.lines().next().is_some() && b.contains("headless]"), "the usage line names the variant");
}

#[test]
fn post_build_installs_the_cloud_service_file_for_this_variant_only_and_fails_without_it() {
    let pb = read("buildroot/board/iyakki/post-build.sh");
    let at = pb.find("IYAKKI_HEADLESS:-0").expect("the headless step");
    let step = &pb[at..pb[at..].find("\nfi\n").map(|e| at + e).unwrap()];
    assert!(step.contains("service/service.cloud.json") && step.contains("/etc/iyakki/service.json") && step.contains("exit 1"), "{step}");
    // no account, no tenant and no password is made in the image
    assert!(!step.contains("user add") && !step.contains("--password"), "{step}");
}

#[test]
fn changed_sh_composes_the_same_defconfig_as_build_sh() {
    let c = read("buildroot/changed.sh");
    assert!(c.contains("[ \"$VARIANT\" = headless ] && cat \"$CFG/iyakki_headless.fragment\""));
    let b = read("buildroot/build.sh");
    assert!(b.contains("cat \"$HERE/configs/iyakki_headless.fragment\" >> \"$OUT/defconfig\""));
}

#[test]
fn the_service_file_of_the_image_is_the_cloud_profile_with_tls_and_a_token_store_and_serves_with_no_arguments() {
    let text = read("service/service.cloud.json");
    let cfg = iyakki_agent::webservice::config::ServiceConfig::parse(&text).expect("it parses");
    let chk = cfg.check().expect("it is valid");
    assert_eq!(chk.profile, iyakki_agent::webservice::Profile::Cloud);
    assert!(cfg.tls.cert.starts_with("/etc/iyakki/") && cfg.tls.key.starts_with("/etc/iyakki/"), "the certificate is kept with the image's configuration");
    assert!(cfg.data_dir.starts_with("/var/lib/iyakki") && cfg.token_store.starts_with("/var/lib/iyakki"), "state goes to the data volume");
    // an init started with no arguments and no kernel option serves exactly this file's profile
    let ask = iyakki_core::headlessboot::image_ask(&[], &|_| None, "", true).unwrap().unwrap();
    assert_eq!((ask.profile, ask.listen), (None, None));
    // the same program, asked for another profile on the kernel line, serves that one (a server with a data-only role)
    let ask = iyakki_core::headlessboot::image_ask(&[], &|_| None, "ro iyakki.profile=data-api", true).unwrap().unwrap();
    assert_eq!(ask.profile.as_deref(), Some("data-api"));
}

#[test]
fn the_size_script_measures_the_headless_program_against_the_full_one_and_the_image_files() {
    let s = read("buildroot/headless-size.sh");
    assert!(s.contains("--no-default-features --features headless") && s.contains("--release") && s.contains("headless is $((100 * h / f)) percent"));
    for img in ["*.iso", "*.img", "*.raw"] {
        assert!(s.contains(img), "{img}");
    }
}
