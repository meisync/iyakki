//! Route 3 for Apple Silicon (F-APPLE-02): a light Linux virtual machine on macOS that runs k3s, as a recipe the person runs on the Mac.
//!
//! IYAKKI runs nothing on the Mac and ships nothing for it. This module writes two plain texts from checked settings: a Lima
//! (Apache-2.0) machine file that starts a stock Debian 12 arm64 cloud image with Apple's Virtualization framework (`vmType: vz`) and
//! installs k3s inside it, and the few `limactl` commands to start, check and remove it. Lima is the person's own download (the
//! installer-only rule). The IYAKKI aarch64 disk image is not used here: it is not a cloud image (no Lima guest agent, no cloud-init, no
//! SSH user), so Lima could never reach it (KI-1015). IYAKKI's own k3s layer is not in it either; the machine is a plain k3s host.
//!
//! Everything downloaded is pinned and checked, and the recipe fails closed: the Debian image by the SHA-512 Lima's own template and
//! Debian's published SHA512SUMS give (Lima checks it), the k3s binary by a SHA-256 and the k3s installer script by a SHA-256, both
//! checked inside the guest before anything runs (a mismatch stops the install). Rows: `legal/downloads.txt` (`applevm-*`).
//!
//! The k3s API is forwarded to the Mac's loopback only; no folder of the Mac is shared into the machine; the guest reaches the Mac (for
//! route 2, a model on Metal) at Lima's `host.lima.internal`; memory and CPU are the limits the person set. Every value is checked first
//! so a setting can never inject YAML or a shell command.

/// Debian 12 (bookworm) generic cloud image for arm64, the release Lima's own `debian-12` template pins (checked against Debian's
/// SHA512SUMS of the same folder on 2026-11-19).
pub const IMAGE_URL: &str = "https://cloud.debian.org/images/cloud/bookworm/20261026-2643/debian-12-genericcloud-arm64-20261026-2643.qcow2";
pub const IMAGE_SHA512: &str = "f42866fe22b4baf9f0ed4f81d620064f2a4b86b18b92bfe3aaaa40babf6819d39cbf0caaaad13f4ae3e7c9f82cbd78b0c8cabadd8bc737ce13578e61fa86970c";
/// k3s, one exact release; the binary and the installer script that belongs to that tag.
pub const K3S_TAG: &str = "v1.37.2+k3s1";
pub const K3S_BINARY_URL: &str = "https://github.com/k3s-io/k3s/releases/download/v1.37.2%2Bk3s1/k3s-arm64";
pub const K3S_BINARY_SHA256: &str = "a779ccf1e5e2e33a62f5a29c2f1c0bbf99431e100579d4c10f0aa0742a6aa84b";
pub const K3S_INSTALLER_URL: &str = "https://raw.githubusercontent.com/k3s-io/k3s/v1.37.2%2Bk3s1/install.sh";
pub const K3S_INSTALLER_SHA256: &str = "ed01f89fd977bf20ac1516bbebf8370bf3ddbaa55dac8aba610956a4c78cc00b";

fn hex(s: &str, n: usize) -> bool {
    s.len() == n && s.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

/// Refuses to write anything when a pin is missing or malformed (fail closed).
pub fn pins_ok() -> Result<(), String> {
    if !hex(IMAGE_SHA512, 128) {
        return Err("the Debian image has no valid SHA-512 pin: refusing to write a recipe that would start an unchecked image".into());
    }
    if !hex(K3S_BINARY_SHA256, 64) || !hex(K3S_INSTALLER_SHA256, 64) {
        return Err("the k3s download has no valid SHA-256 pin: refusing to write a recipe that would install unchecked code".into());
    }
    if !IMAGE_URL.starts_with("https://") || !K3S_BINARY_URL.starts_with("https://") || !K3S_INSTALLER_URL.starts_with("https://") {
        return Err("every download address must be https".into());
    }
    Ok(())
}

/// What the person chooses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Machine name: lower-case letters, digits and dashes.
    pub name: String,
    pub cpus: u32,
    pub memory_gib: u32,
    pub disk_gib: u32,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { name: "iyakki-k3s".into(), cpus: 4, memory_gib: 8, disk_gib: 60 }
    }
}

fn plain(s: &str) -> bool {
    !s.is_empty() && s.len() <= 512 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '+' | '@' | ' ')) && !s.contains("..")
}

impl Settings {
    pub fn check(&self) -> Result<(), String> {
        if self.name.is_empty() || self.name.len() > 32 || !self.name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') || self.name.starts_with('-') {
            return Err("the machine name may hold lower-case letters, digits and dashes (at most 32)".into());
        }
        if !(1..=32).contains(&self.cpus) {
            return Err("cpus must be 1 to 32".into());
        }
        if !(2..=256).contains(&self.memory_gib) {
            return Err("memory must be 2 to 256 GiB (it is taken from the Mac while the machine runs)".into());
        }
        if !(10..=2000).contains(&self.disk_gib) {
            return Err("disk must be 10 to 2000 GiB".into());
        }
        pins_ok()
    }
}

/// The Lima machine file.
pub fn lima_yaml(s: &Settings) -> Result<String, String> {
    s.check()?;
    Ok(format!(
        "# IYAKKI k3s machine for macOS on Apple Silicon (F-APPLE-02): a stock Debian 12 arm64 cloud image + k3s {k3s}.\n\
         # Start it with: limactl start --name={name} this-file.yaml\n\
         vmType: \"vz\"\n\
         arch: \"aarch64\"\n\
         cpus: {cpus}\n\
         memory: \"{mem}GiB\"\n\
         disk: \"{disk}GiB\"\n\
         images:\n\
         \x20 - location: \"{img}\"\n\
         \x20   arch: \"aarch64\"\n\
         \x20   digest: \"sha512:{isha}\"\n\
         mounts: []\n\
         containerd:\n\
         \x20 system: false\n\
         \x20 user: false\n\
         networks:\n\
         \x20 - vzNAT: true\n\
         portForwards:\n\
         \x20 - guestPort: 6443\n\
         \x20   hostIP: \"127.0.0.1\"\n\
         \x20   hostPort: 6443\n\
         \x20   proto: tcp\n\
         provision:\n\
         \x20 - mode: system\n\
         \x20   script: |\n\
         \x20     #!/bin/sh\n\
         \x20     # k3s from two pinned downloads, each checked before it is used; any mismatch stops here (set -e)\n\
         \x20     set -eu\n\
         \x20     [ -x /usr/local/bin/k3s ] && exit 0\n\
         \x20     d=$(mktemp -d)\n\
         \x20     curl -fsSL -o \"$d/k3s\" '{kurl}'\n\
         \x20     echo '{ksha}  '\"$d/k3s\" | sha256sum -c -\n\
         \x20     curl -fsSL -o \"$d/install.sh\" '{iurl}'\n\
         \x20     echo '{insha}  '\"$d/install.sh\" | sha256sum -c -\n\
         \x20     install -m 755 \"$d/k3s\" /usr/local/bin/k3s\n\
         \x20     INSTALL_K3S_SKIP_DOWNLOAD=true INSTALL_K3S_TAG='{k3s}' sh \"$d/install.sh\" server --write-kubeconfig-mode 644\n\
         \x20     rm -rf \"$d\"\n\
         probes:\n\
         \x20 - script: |\n\
         \x20     #!/bin/sh\n\
         \x20     set -eu\n\
         \x20     timeout 120 sh -c 'until test -f /etc/rancher/k3s/k3s.yaml; do sleep 3; done'\n\
         \x20   hint: \"k3s is not up yet: limactl shell {name} sudo journalctl -u k3s\"\n",
        name = s.name,
        cpus = s.cpus,
        mem = s.memory_gib,
        disk = s.disk_gib,
        img = IMAGE_URL,
        isha = IMAGE_SHA512,
        k3s = K3S_TAG,
        kurl = K3S_BINARY_URL,
        ksha = K3S_BINARY_SHA256,
        iurl = K3S_INSTALLER_URL,
        insha = K3S_INSTALLER_SHA256
    ))
}

/// The commands, as a script the person reads and runs in Terminal on the Mac. It installs nothing on the Mac itself; starting the
/// machine downloads the pinned Debian image (Lima, digest checked) and, inside the machine, the pinned k3s.
pub fn script(s: &Settings, yaml_file: &str) -> Result<String, String> {
    s.check()?;
    if !plain(yaml_file) || !yaml_file.starts_with('/') {
        return Err("the machine file path must be absolute".into());
    }
    let q = |t: &str| format!("'{t}'");
    Ok(format!(
        "#!/bin/sh\n\
         # Run on the Mac (Apple Silicon). Needs Lima (limactl), which you download yourself from https://lima-vm.io\n\
         set -eu\n\
         command -v limactl >/dev/null || {{ echo 'limactl (Lima) is not installed: download it from lima-vm.io first'; exit 1; }}\n\
         [ \"$(uname -m)\" = arm64 ] || {{ echo 'this recipe is for Apple Silicon'; exit 1; }}\n\
         case \"${{1:-start}}\" in\n\
         \x20 start)  echo 'Starting downloads the Debian 12 arm64 cloud image (SHA-512 checked by Lima) and, inside the machine, k3s {k3s} (SHA-256 checked); a mismatch stops it.'\n\
         \x20         limactl start --name={name} {yaml}\n\
         \x20         echo 'Check: limactl shell {name} sudo k3s kubectl get nodes' ;;\n\
         \x20 status) limactl list {name} ;;\n\
         \x20 kubeconfig) limactl shell {name} sudo cat /etc/rancher/k3s/k3s.yaml ;;\n\
         \x20 stop)   limactl stop {name} ;;\n\
         \x20 remove) limactl delete --force {name} ;;\n\
         \x20 *) echo 'usage: start | status | kubeconfig | stop | remove'; exit 2 ;;\n\
         esac\n",
        name = s.name,
        k3s = K3S_TAG,
        yaml = q(yaml_file)
    ))
}

/// One plain text for the person: what will happen and the two files.
pub fn recipe(s: &Settings, yaml_file: &str) -> Result<String, String> {
    Ok(format!(
        "A light Linux machine for k3s on your Mac (Apple Silicon), using Apple's Virtualization framework through Lima. It starts a stock Debian 12 arm64 cloud image (not the IYAKKI disk image, which Lima cannot reach) and installs k3s inside it; both downloads are pinned and checked, and a mismatch stops the start.\n\
         It takes {} CPU(s), {} GiB of memory and up to {} GiB of disk from the Mac while it runs. No folder of the Mac is shared into it; the k3s API is reachable on this Mac only (127.0.0.1:6443); models on the Mac's GPU (route 2) are reached from the machine at host.lima.internal.\n\
         This text writes nothing and starts nothing: save the machine file as {yaml_file}, save the script, read both, then run the script. `kubeconfig` prints the cluster's access file (its server address is 127.0.0.1:6443, the forwarded port) for you to save and connect in IYAKKI.\n\n\
         --- machine file ---\n{}\n--- script ---\n{}",
        s.cpus, s.memory_gib, s.disk_gib, lima_yaml(s)?, script(s, yaml_file)?
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok() -> Settings {
        Settings::default()
    }

    #[test]
    fn the_machine_file_uses_the_virtualization_framework_a_pinned_cloud_image_shares_nothing_and_forwards_k3s_to_loopback_only() {
        let y = lima_yaml(&ok()).unwrap();
        for want in ["vmType: \"vz\"", "arch: \"aarch64\"", "cpus: 4", "memory: \"8GiB\"", "disk: \"60GiB\"", "mounts: []", "hostIP: \"127.0.0.1\"", "guestPort: 6443", &format!("digest: \"sha512:{IMAGE_SHA512}\""), &format!("location: \"{IMAGE_URL}\"")] {
            assert!(y.contains(want), "{want} missing in\n{y}");
        }
        assert!(IMAGE_URL.contains("debian-12-genericcloud-arm64") && !y.contains(".img"), "a stock cloud image, never the IYAKKI disk image");
    }

    #[test]
    fn k3s_is_installed_in_the_guest_from_pinned_downloads_that_are_checked_before_use() {
        let y = lima_yaml(&ok()).unwrap();
        for want in [
            "provision:",
            "set -eu",
            &format!("echo '{K3S_BINARY_SHA256}  '\"$d/k3s\" | sha256sum -c -"),
            &format!("echo '{K3S_INSTALLER_SHA256}  '\"$d/install.sh\" | sha256sum -c -"),
            &format!("INSTALL_K3S_SKIP_DOWNLOAD=true INSTALL_K3S_TAG='{K3S_TAG}'"),
            K3S_BINARY_URL,
            K3S_INSTALLER_URL,
        ] {
            assert!(y.contains(want), "{want} missing in\n{y}");
        }
        // the check comes before the install, and no floating address (get.k3s.io, a channel, latest) is used
        assert!(y.find("sha256sum -c").unwrap() < y.find("install -m 755").unwrap() && y.find("install -m 755").unwrap() < y.find("sh \"$d/install.sh\"").unwrap());
        assert!(!y.contains("get.k3s.io") && !y.contains("latest") && !y.contains("| sh\n") && !y.contains("| sh ") && !y.contains("INSTALL_K3S_CHANNEL"));
        assert!(K3S_BINARY_URL.contains("2Bk3s1") && K3S_INSTALLER_URL.contains("2Bk3s1") && K3S_TAG == "v1.37.2+k3s1");
    }

    #[test]
    fn the_recipe_fails_closed_when_a_pin_is_missing_or_malformed() {
        assert!(pins_ok().is_ok());
        assert!(hex(IMAGE_SHA512, 128) && hex(K3S_BINARY_SHA256, 64) && hex(K3S_INSTALLER_SHA256, 64));
        assert!(!hex("", 64) && !hex(&"AB".repeat(32), 64) && !hex(&"zz".repeat(32), 64) && !hex(&"ab".repeat(31), 64));
        for u in [IMAGE_URL, K3S_BINARY_URL, K3S_INSTALLER_URL] {
            assert!(u.starts_with("https://"), "{u}");
        }
    }

    #[test]
    fn every_setting_is_checked_so_nothing_can_inject_yaml_or_shell() {
        let bad = |f: &dyn Fn(&mut Settings)| {
            let mut s = ok();
            f(&mut s);
            lima_yaml(&s).is_err() && script(&s, "/tmp/a.yaml").is_err()
        };
        assert!(bad(&|s| s.name = "A b".into()) && bad(&|s| s.name = "x;rm".into()) && bad(&|s| s.name = String::new()) && bad(&|s| s.name = "-x".into()));
        assert!(bad(&|s| s.cpus = 0) && bad(&|s| s.cpus = 99) && bad(&|s| s.memory_gib = 1) && bad(&|s| s.disk_gib = 5000));
        assert!(script(&ok(), "/tmp/a'; rm -rf ~; '.yaml").is_err() && script(&ok(), "a.yaml").is_err());
    }

    #[test]
    fn the_script_checks_for_lima_and_apple_silicon_and_offers_start_status_kubeconfig_stop_remove_only() {
        let t = script(&ok(), "/Users/me/iyakki-k3s.yaml").unwrap();
        assert!(t.starts_with("#!/bin/sh\n") && t.contains("command -v limactl") && t.contains("= arm64"));
        for c in ["limactl start --name=iyakki-k3s '/Users/me/iyakki-k3s.yaml'", "status) limactl list iyakki-k3s", "kubeconfig) limactl shell iyakki-k3s sudo cat /etc/rancher/k3s/k3s.yaml", "stop)   limactl stop iyakki-k3s", "remove) limactl delete --force iyakki-k3s", "SHA-256 checked"] {
            assert!(t.contains(c), "{c}\n{t}");
        }
        assert!(!t.contains("curl") && !t.contains("brew install") && !t.lines().any(|l| l.trim_start().starts_with("sudo ")), "the script installs nothing on the Mac");
    }

    #[test]
    fn the_recipe_says_what_it_takes_what_it_downloads_and_that_it_writes_nothing() {
        let r = recipe(&ok(), "/Users/me/iyakki-k3s.yaml").unwrap();
        assert!(r.contains("4 CPU(s), 8 GiB") && r.contains("writes nothing and starts nothing") && r.contains("host.lima.internal") && r.contains("--- script ---"));
        assert!(r.contains("stock Debian 12 arm64 cloud image") && r.contains("mismatch stops the start"));
    }
}
