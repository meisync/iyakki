//! Applies a sample ruleset to the kernel of the *current network namespace* (run it under `unshare -rn`, see
//! tests/kernel.rs; never on the real network). `fw_demo apply | remove | counters | text`.
use iyakki_fw::backend::{Backend, KernelBackend};
use iyakki_fw::model::{Config, Kind, Preset, Proto, Rule};
use iyakki_fw::ruleset::{build, Ctx};

fn main() {
    let mut cfg = Config::default();
    cfg.set_preset(Preset::Ssh, true);
    cfg.set_preset(Preset::SharedFolders, true);
    let mut r = Rule::new(Kind::Cidr, "203.0.113.0/24");
    r.proto = Proto::Tcp;
    r.ports = "80,443,8000-8100".into();
    cfg.add(r);
    cfg.add(Rule::new(Kind::Range, "198.51.100.10-198.51.100.20"));
    cfg.add(Rule::new(Kind::Ip, "2001:db8::5"));
    cfg.add(Rule::new(Kind::Cidr, "2001:db8:1::/48"));
    let mut host = Rule::new(Kind::Host, "files.example.com");
    host.proto = Proto::Udp;
    host.ports = "53".into();
    cfg.add(host);
    let mut resolved = iyakki_fw::model::Resolved::default();
    resolved.addrs.insert("files.example.com".into(), vec!["192.0.2.7".parse().unwrap()]);
    let built = build(&cfg, &resolved, &Ctx { now: 1, ssh_boot: true, ..Ctx::default() }).unwrap();
    let mut k = KernelBackend::new();
    match std::env::args().nth(1).as_deref() {
        Some("apply") => k.replace(&built.ruleset).unwrap_or_else(|e| {
            eprintln!("apply failed: {e}");
            std::process::exit(1)
        }),
        Some("remove") => k.remove().unwrap_or_else(|e| {
            eprintln!("remove failed: {e}");
            std::process::exit(1)
        }),
        Some("watch") => {
            let log = iyakki_fw::blocked::BlockLog::new();
            iyakki_fw::blocked::start(log.clone(), iyakki_fw::ruleset::LOG_GROUP, || 1).unwrap_or_else(|e| {
                eprintln!("log failed: {e}");
                std::process::exit(1)
            });
            std::thread::sleep(std::time::Duration::from_secs(5));
            for b in log.recent() {
                println!("blocked {} {} port {} x{}", b.src, b.proto_name(), b.dport, b.count);
            }
            log.stop();
        }
        Some("counters") => println!("{:?}", k.counters().unwrap()),
        _ => print!("{}", built.ruleset.to_text()),
    }
}
