//! Build-time step of F-PKG-11: `iyakki-modpack ROOT RELEASE [--pack-out DIR]`. Run after the modules are signed and
//! before `depmod`. Touches only ROOT (and DIR); prints what it did.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (root, rel) = match (a.first(), a.get(1)) {
        (Some(r), Some(v)) => (r.clone(), v.clone()),
        _ => {
            eprintln!("usage: iyakki-modpack ROOT RELEASE [--pack-out DIR]");
            std::process::exit(2);
        }
    };
    let out = a.iter().position(|x| x == "--pack-out").and_then(|i| a.get(i + 1)).map(std::path::PathBuf::from);
    match iyakki_drivers::modsplit::compress_tree(std::path::Path::new(&root), &rel, out.as_deref()) {
        Ok(r) => println!("modpack: {} compressed, {} left alone, {} moved to the optional pack, {} -> {} bytes", r.compressed, r.skipped, r.moved_out, r.bytes_before, r.bytes_after),
        Err(e) => {
            eprintln!("modpack: {e}");
            std::process::exit(1);
        }
    }
}
