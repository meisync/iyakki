//! What the kernel says about NVIDIA cards (F-ARCH-07), moved out of the agent's `gpu_nvidia.rs`: the GPUs and the version the loaded driver reports
//! (`/proc/driver/nvidia`), which `/dev/nvidia*` nodes exist, the nodes `nvidia-modprobe` would make, and the runtime power switch of the card's PCI
//! functions. The agent keeps what is the installer's own (download, staging, the CDI spec, the state machine) and asks this module for the facts.
//! Every function takes the file system root so a test uses a made-up tree.

use std::path::Path;

/// NVIDIA's character device major.
pub const NVIDIA_MAJOR: u32 = 195;

/// The device nodes a container may be given, in the order the CDI spec lists them.
pub const NODE_NAMES: [&str; 4] = ["/dev/nvidiactl", "/dev/nvidia-uvm", "/dev/nvidia-uvm-tools", "/dev/nvidia-modeset"];

/// The control node: without it the driver is not loaded or its nodes were not made.
pub const CONTROL_NODE: &str = "/dev/nvidiactl";

/// The node of the GPU with this minor.
pub fn gpu_node(minor: u32) -> String {
    format!("/dev/nvidia{minor}")
}

/// A device node to make: `mknod path c major minor`, mode 0666.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeToMake {
    pub path: String,
    pub major: u32,
    pub minor: u32,
}

/// The nodes `nvidia-modprobe` would make: `/dev/nvidiactl` (195, 255), `/dev/nvidia<minor>` (195, minor) for each GPU and the UVM nodes
/// (the major of `nvidia-uvm` from `/proc/devices`; minor 0 and 1). `gpu_minors` come from `/proc/driver/nvidia/gpus/*/information`
/// (`Device Minor: N`); `proc_devices` is the text of `/proc/devices`.
pub fn nodes_to_make(gpu_minors: &[u32], proc_devices: &str) -> Vec<NodeToMake> {
    let mut v = vec![NodeToMake { path: CONTROL_NODE.into(), major: NVIDIA_MAJOR, minor: 255 }];
    for m in gpu_minors {
        v.push(NodeToMake { path: gpu_node(*m), major: NVIDIA_MAJOR, minor: *m });
    }
    if let Some(uvm) = device_major(proc_devices, "nvidia-uvm") {
        v.push(NodeToMake { path: "/dev/nvidia-uvm".into(), major: uvm, minor: 0 });
        v.push(NodeToMake { path: "/dev/nvidia-uvm-tools".into(), major: uvm, minor: 1 });
    }
    v
}

/// The major number of a character device in `/proc/devices` text.
pub fn device_major(proc_devices: &str, name: &str) -> Option<u32> {
    let mut chars = false;
    for l in proc_devices.lines() {
        if l.starts_with("Character devices") {
            chars = true;
            continue;
        }
        if l.starts_with("Block devices") {
            chars = false;
        }
        if chars {
            let mut w = l.split_whitespace();
            if let (Some(n), Some(d)) = (w.next(), w.next()) {
                if d == name {
                    return n.parse().ok();
                }
            }
        }
    }
    None
}

/// `Device Minor: N` of one `/proc/driver/nvidia/gpus/<pci>/information`.
pub fn parse_minor(info: &str) -> Option<u32> {
    info.lines().find_map(|l| l.strip_prefix("Device Minor:").and_then(|v| v.trim().parse().ok()))
}

/// `GPU UUID: GPU-xxxx` of the same file.
pub fn parse_uuid(info: &str) -> Option<String> {
    info.lines().find_map(|l| l.strip_prefix("GPU UUID:").map(|v| v.trim().to_string()))
}

/// The minors and UUIDs of the GPUs the loaded driver found, sorted by minor: reads `<root>/proc/driver/nvidia/gpus/*/information`.
pub fn driver_gpus(root: &Path) -> Vec<(u32, Option<String>)> {
    let mut v: Vec<(u32, Option<String>)> = Vec::new();
    for e in std::fs::read_dir(root.join("proc/driver/nvidia/gpus")).into_iter().flatten().flatten() {
        if let Ok(t) = std::fs::read_to_string(e.path().join("information")) {
            if let Some(m) = parse_minor(&t) {
                v.push((m, parse_uuid(&t)));
            }
        }
    }
    v.sort();
    v
}

/// The version of the loaded driver (`/proc/driver/nvidia/version`: `NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  595.99.02 ...`).
pub fn loaded_version(root: &Path) -> Option<String> {
    let t = std::fs::read_to_string(root.join("proc/driver/nvidia/version")).ok()?;
    t.split_whitespace().find(|w| w.len() >= 5 && w.chars().all(|c| c.is_ascii_digit() || c == '.') && w.matches('.').count() >= 1 && w.chars().next().is_some_and(|c| c.is_ascii_digit()) && w.split('.').next().is_some_and(|m| m.len() == 3)).map(str::to_string)
}

/// Which of `names` (absolute device paths such as `/dev/nvidiactl`) exist under `root`.
pub fn nodes_present(root: &Path, names: &[String]) -> Vec<String> {
    names.iter().filter(|n| root.join(n.trim_start_matches('/')).exists()).cloned().collect()
}

/// Let the kernel power the cards at `pci` (display functions such as `0000:01:00.0`) down when idle: `power/control` = `auto` on each of them and on
/// the other functions of the same slot (the HDMI audio at `.1`, the USB-C controller). Returns what was changed, as PCI addresses.
pub fn allow_power_saving(root: &Path, pci: &[String]) -> Vec<String> {
    let dir = root.join("sys/bus/pci/devices");
    let mut changed = Vec::new();
    for g in pci {
        let slot = g.rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or_else(|| g.clone());
        let mut names: Vec<String> = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with(&format!("{slot}."))).collect();
        names.sort();
        for n in names {
            let ctl = dir.join(&n).join("power/control");
            if std::fs::read_to_string(&ctl).map(|t| t.trim() != "auto").unwrap_or(false) && std::fs::write(&ctl, "auto\n").is_ok() {
                changed.push(n);
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-nv-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn gpus_and_the_loaded_version_are_read_from_the_driver_folder() {
        let d = tree("gpus");
        for (pci, minor, uuid) in [("0000:02:00.0", 1, "GPU-bbb"), ("0000:01:00.0", 0, "GPU-aaa")] {
            let f = d.join("proc/driver/nvidia/gpus").join(pci);
            std::fs::create_dir_all(&f).unwrap();
            std::fs::write(f.join("information"), format!("Model: x\nDevice Minor: {minor}\nGPU UUID: {uuid}\n")).unwrap();
        }
        assert_eq!(driver_gpus(&d), vec![(0, Some("GPU-aaa".to_string())), (1, Some("GPU-bbb".to_string()))]);
        std::fs::write(d.join("proc/driver/nvidia/version"), "NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  595.99.02  Release Build\n").unwrap();
        assert_eq!(loaded_version(&d).as_deref(), Some("595.99.02"));
        assert!(driver_gpus(&d.join("nothing")).is_empty() && loaded_version(&d.join("nothing")).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn nodes_are_made_for_the_control_file_each_gpu_and_the_uvm_pair() {
        let devices = "Character devices:\n  1 mem\n511 nvidia-uvm\n\nBlock devices:\n  8 sd\n";
        let n = nodes_to_make(&[0, 1], devices);
        assert_eq!(n.iter().map(|x| x.path.as_str()).collect::<Vec<_>>(), ["/dev/nvidiactl", "/dev/nvidia0", "/dev/nvidia1", "/dev/nvidia-uvm", "/dev/nvidia-uvm-tools"]);
        assert_eq!(n[3].major, 511);
        assert_eq!(nodes_to_make(&[0], "Character devices:\n").len(), 2, "no uvm major: no uvm nodes");
    }

    #[test]
    fn present_nodes_are_the_ones_under_the_root() {
        let d = tree("nodes");
        std::fs::create_dir_all(d.join("dev")).unwrap();
        std::fs::write(d.join("dev/nvidiactl"), "").unwrap();
        let want: Vec<String> = NODE_NAMES.iter().map(|s| s.to_string()).collect();
        assert_eq!(nodes_present(&d, &want), vec!["/dev/nvidiactl".to_string()]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn power_saving_is_switched_on_for_the_card_and_its_slot_mates_and_reported_once() {
        let d = tree("power");
        for f in ["0000:01:00.0", "0000:01:00.1", "0000:02:00.0"] {
            let p = d.join("sys/bus/pci/devices").join(f).join("power");
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("control"), "on\n").unwrap();
        }
        let changed = allow_power_saving(&d, &["0000:01:00.0".to_string()]);
        assert_eq!(changed, ["0000:01:00.0", "0000:01:00.1"]);
        assert_eq!(std::fs::read_to_string(d.join("sys/bus/pci/devices/0000:02:00.0/power/control")).unwrap(), "on\n", "another slot is left alone");
        assert!(allow_power_saving(&d, &["0000:01:00.0".to_string()]).is_empty(), "already auto: nothing changed");
        let _ = std::fs::remove_dir_all(&d);
    }
}
