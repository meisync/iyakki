//! Prints the F-APPLE-02 Lima machine file and script from the repo's generator:
//! `cargo run -p iyakki-agent --example applevm_print -- <yaml path> <out dir>`.
use iyakki_agent::applevm::{lima_yaml, script, Settings};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() != 3 {
        eprintln!("usage: applevm_print <yaml path> <out dir>");
        std::process::exit(2);
    }
    let s = Settings { cpus: 4, memory_gib: 8, disk_gib: 40, ..Settings::default() };
    let out = std::path::Path::new(&a[2]);
    std::fs::create_dir_all(out).expect("out dir");
    std::fs::write(out.join("iyakki-k3s.yaml"), lima_yaml(&s).expect("yaml")).expect("write");
    std::fs::write(out.join("iyakki-k3s.sh"), script(&s, &a[1]).expect("script")).expect("write");
}
