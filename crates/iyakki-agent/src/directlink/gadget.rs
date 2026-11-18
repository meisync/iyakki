//! USB gadget mode: on a computer whose USB controller can act as a device (`/sys/class/udc` is not empty: ARM boards, phones, some
//! mini PCs with an OTG port, never an ordinary PC), make it look like a USB network card (CDC NCM, or ECM for older hosts) through
//! configfs, so the computer on the other end of the cable sees a normal network interface (`usb0` here, `enx...` there).
//! The plan is a list of directory and file operations under `/sys/kernel/config/usb_gadget/iyakki`; [`apply`] runs it through a
//! small [`Fs`] seam. Needs a kernel with CONFIG_USB_GADGET, CONFIG_USB_CONFIGFS and the NCM/ECM functions, and a device-capable
//! controller driver: it is only done when an administrator asks (`iyakki dev usb gadget on`), never at boot.

pub const ROOT: &str = "/sys/kernel/config/usb_gadget/iyakki";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Mkdir(String),
    Write(String, String),
    Symlink(String, String),
    Rmdir(String),
    Remove(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Function {
    Ncm,
    Ecm,
}

fn f(path: &str) -> String {
    format!("{ROOT}/{path}")
}

/// Locally administered, unicast MAC addresses for the two ends, made from the host name's hash so they stay the same between runs.
pub fn macs(seed: &str) -> (String, String) {
    let mut h: u32 = 0x811c9dc5;
    for b in seed.bytes() {
        h = (h ^ u32::from(b)).wrapping_mul(0x01000193);
    }
    let b = h.to_be_bytes();
    (format!("02:49:59:{:02x}:{:02x}:01", b[0], b[1]), format!("02:49:59:{:02x}:{:02x}:02", b[2], b[3]))
}

/// Everything to write to switch the gadget on, in order. The last step binds it to the controller `udc`.
pub fn plan_on(udc: &str, func: Function, host_name: &str) -> Result<Vec<Op>, String> {
    if udc.is_empty() || !udc.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':')) {
        return Err("not a USB device controller name".into());
    }
    let fname = match func {
        Function::Ncm => "ncm.usb0",
        Function::Ecm => "ecm.usb0",
    };
    let (host_mac, dev_mac) = macs(host_name);
    Ok(vec![
        Op::Mkdir(ROOT.into()),
        // Linux Foundation's "Multifunction Composite Gadget" ids are for this kind of use; the strings say what it is
        Op::Write(f("idVendor"), "0x1d6b".into()),
        Op::Write(f("idProduct"), "0x0104".into()),
        Op::Write(f("bcdUSB"), "0x0200".into()),
        Op::Mkdir(f("strings/0x409")),
        Op::Write(f("strings/0x409/manufacturer"), "IYAKKI".into()),
        Op::Write(f("strings/0x409/product"), "IYAKKI direct link".into()),
        Op::Mkdir(f("configs/c.1")),
        Op::Mkdir(f("configs/c.1/strings/0x409")),
        Op::Write(f("configs/c.1/strings/0x409/configuration"), "network".into()),
        Op::Mkdir(f(&format!("functions/{fname}"))),
        Op::Write(f(&format!("functions/{fname}/host_addr")), host_mac),
        Op::Write(f(&format!("functions/{fname}/dev_addr")), dev_mac),
        Op::Symlink(f(&format!("functions/{fname}")), f(&format!("configs/c.1/{fname}"))),
        Op::Write(f("UDC"), udc.into()),
    ])
}

/// Everything to undo it, in the order the kernel needs.
pub fn plan_off() -> Vec<Op> {
    let mut v = vec![Op::Write(f("UDC"), String::new())];
    for fname in ["ncm.usb0", "ecm.usb0"] {
        v.push(Op::Remove(f(&format!("configs/c.1/{fname}"))));
    }
    v.push(Op::Rmdir(f("configs/c.1/strings/0x409")));
    v.push(Op::Rmdir(f("configs/c.1")));
    for fname in ["ncm.usb0", "ecm.usb0"] {
        v.push(Op::Rmdir(f(&format!("functions/{fname}"))));
    }
    v.push(Op::Rmdir(f("strings/0x409")));
    v.push(Op::Rmdir(ROOT.into()));
    v
}

/// The file system seam.
pub trait Fs {
    fn mkdir(&self, p: &str) -> Result<(), String>;
    fn write(&self, p: &str, v: &str) -> Result<(), String>;
    fn symlink(&self, to: &str, at: &str) -> Result<(), String>;
    fn rmdir(&self, p: &str) -> Result<(), String>;
    fn remove(&self, p: &str) -> Result<(), String>;
}

/// Run a plan. On the way on, a failure undoes what was made. On the way off, every step is tried (a missing one is not an error).
pub fn apply(fs: &dyn Fs, plan: &[Op], undo_on_error: bool) -> Result<(), String> {
    for op in plan {
        let r = match op {
            Op::Mkdir(p) => fs.mkdir(p),
            Op::Write(p, v) => fs.write(p, v),
            Op::Symlink(to, at) => fs.symlink(to, at),
            Op::Rmdir(p) => fs.rmdir(p),
            Op::Remove(p) => fs.remove(p),
        };
        if let Err(e) = r {
            if undo_on_error {
                let _ = apply(fs, &plan_off(), false);
                return Err(format!("{e} (the gadget was taken apart again)"));
            }
        }
    }
    Ok(())
}

/// The real configfs.
pub struct RealFs;

impl Fs for RealFs {
    fn mkdir(&self, p: &str) -> Result<(), String> {
        guard(p)?;
        std::fs::create_dir(p).or_else(|e| if e.kind() == std::io::ErrorKind::AlreadyExists { Ok(()) } else { Err(format!("{p}: {e}")) })
    }
    fn write(&self, p: &str, v: &str) -> Result<(), String> {
        guard(p)?;
        std::fs::write(p, v).map_err(|e| format!("{p}: {e}"))
    }
    #[cfg(unix)]
    fn symlink(&self, to: &str, at: &str) -> Result<(), String> {
        guard(at)?;
        std::os::unix::fs::symlink(to, at).map_err(|e| format!("{at}: {e}"))
    }
    #[cfg(not(unix))]
    fn symlink(&self, _: &str, _: &str) -> Result<(), String> {
        Err("not available here".into())
    }
    fn rmdir(&self, p: &str) -> Result<(), String> {
        guard(p)?;
        std::fs::remove_dir(p).map_err(|e| format!("{p}: {e}"))
    }
    fn remove(&self, p: &str) -> Result<(), String> {
        guard(p)?;
        std::fs::remove_file(p).map_err(|e| format!("{p}: {e}"))
    }
}

/// Only paths under the gadget's own folder are ever touched.
fn guard(p: &str) -> Result<(), String> {
    if p.starts_with(ROOT) && !p.contains("..") { Ok(()) } else { Err(format!("{p}: outside the gadget folder")) }
}

/// The first USB device controller, from `/sys/class/udc`.
pub fn first_udc(sys_class_udc: &std::path::Path) -> Option<String> {
    let mut v: Vec<String> = std::fs::read_dir(sys_class_udc).ok()?.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v.into_iter().next()
}

#[cfg(test)]
pub fn guard_for_test(p: &str) -> Result<(), String> {
    guard(p)
}
