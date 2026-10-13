//! `cargo run -p iyakki-netscan --example scan -- 192.168.1.0/24` (no argument: this machine's own networks)
fn main() {
    let arg = std::env::args().nth(1);
    let targets = match arg {
        Some(a) => iyakki_netscan::net::parse_targets(&a).unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(2)
        }),
        None => iyakki_netscan::net::local_subnets().iter().flat_map(iyakki_netscan::net::hosts_of).collect(),
    };
    eprintln!("scanning {} addresses…", targets.len());
    let hosts = iyakki_netscan::scan_blocking(targets, iyakki_netscan::Options::default());
    for h in &hosts {
        println!("{}  {}  {}", h.ip, h.mac.as_deref().unwrap_or("-"), h.name.as_deref().unwrap_or(""));
        for s in &h.services {
            println!("   {:>5}/{}  {:<12} {}{}{}", s.port, s.proto, s.kind.label(), s.name, if s.detail.is_empty() { String::new() } else { format!("  — {}", s.detail) }, s.risk.as_ref().map(|r| format!("  ⚠ {r}")).unwrap_or_default());
        }
    }
    eprintln!("{} hosts answered", hosts.len());
}
