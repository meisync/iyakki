//! `iyakki dev usb ...`: check, pair and measure a one-cable connection to another computer (see `docs/DEVELOPER-MODE.md`).
//!
//! ```text
//! iyakki dev usb [check]                  the seven steps, each with what to do when it fails
//! iyakki dev usb authorize                approve the waiting Thunderbolt device (administrator on this computer, root)
//! iyakki dev usb pair [--code CODE]       pair with the other computer (same pairing as `iyakki dev pair`; the code is on its screen)
//! iyakki dev usb speed                    send 64 MiB through the developer-mode link and show Gbit/s and delay
//! iyakki dev usb link [--fixed-pair on|off] [--allow-more on|off]    settings of the link (root)
//! iyakki dev usb gadget on|off            act as a USB network card (needs a USB device controller; root)
//! ```

use std::path::Path;

use super::checks::{self, State};
use super::{gadget, Env};

pub const USAGE: &str = "usage: iyakki dev usb [check] | authorize | pair [--code CODE] | speed | link [--fixed-pair on|off] [--allow-more on|off] | gadget on|off";

/// Run against any machine (the tests pass a fake). `read_line` asks the person for a line (the pairing code).
pub fn run_with(env: &dyn Env, a: &[String], out: &mut dyn FnMut(&str), read_line: &dyn Fn(&str) -> String) -> Result<(), String> {
    let sub = a.first().map(String::as_str).unwrap_or("check");
    let flag = |k: &str| a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).map(String::as_str);
    match sub {
        "check" | "" => {
            show(env, &checks::run(env, None), out);
            Ok(())
        }
        "authorize" => {
            let r = checks::run(env, None);
            out(&checks::authorize_pending(env, &r)?);
            Ok(())
        }
        "pair" => {
            let r = checks::run(env, None);
            let code = match flag("--code") {
                Some(c) => c.to_string(),
                None => {
                    if let Some(p) = &r.peer {
                        out(&format!("Other computer: {} at {}", p.beacon.host, p.addr));
                    }
                    if let Ok(h) = r.peer.as_ref().map(|p| env.dev_hello(p.addr)).unwrap_or(Err("no other computer found (run `iyakki dev usb` to see why)".into())) {
                        out(&format!("Its certificate fingerprint: {}\nCheck that the other computer's screen shows the same fingerprint.", h.fingerprint));
                    }
                    read_line("Pairing code shown on the other computer: ")
                }
            };
            out(&checks::pair(env, &r, &code)?);
            Ok(())
        }
        "speed" => {
            let r = checks::run(env, None);
            let s = checks::speed_test(env, &r)?;
            out(&format!("{:.2} Gbit/s ({} MiB in {:.2} s){}", s.sample.gbit_per_s(), s.sample.bytes >> 20, s.sample.secs, s.rtt_ms.map(|m| format!(", delay {m:.2} ms")).unwrap_or_default()));
            Ok(())
        }
        "link" => {
            let mut c = env.config();
            let on = |v: &str| match v {
                "on" | "yes" | "1" => Ok(true),
                "off" | "no" | "0" => Ok(false),
                _ => Err(format!("{v:?}: say on or off")),
            };
            if let Some(v) = flag("--fixed-pair") {
                c.fixed_pair = on(v)?;
            }
            if let Some(v) = flag("--allow-more") {
                c.allow_more = on(v)?;
            }
            if flag("--fixed-pair").is_some() || flag("--allow-more").is_some() {
                env.save_config(&c)?;
            }
            out(&format!("fixed private pair (10.77.0.1 / 10.77.0.2): {}\nallow more than developer mode and ping on direct links: {}", if c.fixed_pair { "on" } else { "off" }, if c.allow_more { "on (everything is let in on the cable)" } else { "off" }));
            Ok(())
        }
        "gadget" => match a.get(1).map(String::as_str) {
            Some(w @ ("on" | "off")) => gadget_run(w == "on", Path::new("/sys/class/udc"), &gadget::RealFs, out),
            _ => Err("usage: iyakki dev usb gadget on|off (on the computer that has the USB device controller)".into()),
        },
        _ => Err(USAGE.into()),
    }
}

/// Switch the gadget on or off (a fake `Fs` and a copy of `/sys/class/udc` in the tests).
pub fn gadget_run(on: bool, udc_dir: &Path, fs: &dyn gadget::Fs, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    if on {
        let udc = gadget::first_udc(udc_dir).ok_or("this computer has no USB device controller (/sys/class/udc is empty): an ordinary PC cannot do this. Use a Thunderbolt/USB4 cable or a host-to-host bridge cable instead.")?;
        let host = std::fs::read_to_string("/etc/hostname").unwrap_or_default();
        gadget::apply(fs, &gadget::plan_on(&udc, gadget::Function::Ncm, host.trim())?, true).map_err(|e| format!("{e}. Needs the kernel's libcomposite and configfs: modprobe libcomposite; mount -t configfs none /sys/kernel/config; and a kernel built with CONFIG_USB_CONFIGFS_NCM."))?;
        out(&format!("The USB gadget is on ({udc}): the other computer now sees a USB network card; here the interface is usb0."));
    } else {
        gadget::apply(fs, &gadget::plan_off(), false)?;
        out("The USB gadget is off.");
    }
    Ok(())
}

fn show(env: &dyn Env, r: &checks::Report, out: &mut dyn FnMut(&str)) {
    for c in &r.checks {
        out(&format!("[{}] {}", match c.state { State::Pass => " ok ", State::Fail => "FAIL", State::Wait => " .. " }, c.title));
        if !c.text.is_empty() {
            out(&format!("       {}", c.text));
        }
    }
    out(&format!("\nLast result: {}", r.last));
    let _ = env;
}

/// `iyakki dev usb ...` on this computer.
pub fn run(a: &[String], home: &Path, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let data = std::env::var("IYAKKI_DATA").unwrap_or_else(|_| "/var/lib/iyakki".into());
    let env = super::real::RealEnv::new(Path::new(&data), home);
    run_with(&env, a, out, &|prompt| {
        use std::io::Write;
        print!("{prompt}");
        let _ = std::io::stdout().flush();
        let mut l = String::new();
        let _ = std::io::stdin().read_line(&mut l);
        l.trim().to_string()
    })
}
