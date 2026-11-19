//! Bluetooth through the kernel's management interface: `btctl list | info N | on N | off N | scan N [secs]`.
//! Needs root. Meant for trying a controller on the real machine; the Settings app uses the same library.
use iyakki_devctl::btmgmt::{ControlSocket, Mgmt};
use std::time::Duration;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let sock = ControlSocket::open().unwrap_or_else(|e| {
        eprintln!("cannot open the Bluetooth control channel: {e} (root? is the bluetooth module loaded?)");
        std::process::exit(1)
    });
    let mut m = Mgmt::new(sock);
    let idx = |n: usize| a.get(n).and_then(|s| s.parse::<u16>().ok()).unwrap_or(0);
    let r: Result<(), iyakki_devctl::btmgmt::Error> = (|| {
        match a.first().map(String::as_str) {
            Some("list") | None => println!("controllers: {:?}", m.index_list()?),
            Some("info") => println!("{:?}", m.info(idx(1))?),
            Some("on") => println!("settings {:#x}", m.set_powered(idx(1), true)?),
            Some("off") => println!("settings {:#x}", m.set_powered(idx(1), false)?),
            Some("scan") => {
                let secs = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(8);
                m.prepare(idx(1))?;
                for f in m.scan(idx(1), Duration::from_secs(secs))? {
                    println!("{} {:4} dBm {:?} {}", f.address, f.rssi, f.kind, f.name);
                }
            }
            Some(o) => eprintln!("unknown command {o}"),
        }
        Ok(())
    })();
    if let Err(e) = r {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
