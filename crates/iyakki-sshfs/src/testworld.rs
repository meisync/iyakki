//! A small file tree with links that lead out of it, and the `Jail` over it: shared by this crate's tests and the agent's SFTP manager
//! tests (feature `testing`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::sftp_fs::{Jail, Rule};

pub type Log = Arc<Mutex<Vec<String>>>;

pub struct World {
    pub root: PathBuf,
    pub home: PathBuf,
    pub shared: PathBuf,
    pub outside: PathBuf,
}

pub fn world(tag: &str) -> (World, Jail) {
    let root = std::fs::canonicalize(crate::testdir::unique("sftp", tag)).unwrap();
    let (home, shared, outside) = (root.join("home/amy"), root.join("shared"), root.join("outside"));
    for d in [&home, &shared, &outside, &home.join("sub")] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(home.join("a.txt"), "alpha").unwrap();
    std::fs::write(home.join("sub/deep.txt"), "deep").unwrap();
    std::fs::write(shared.join("pub.txt"), "public").unwrap();
    std::fs::write(outside.join("secret.txt"), "TOP SECRET").unwrap();
    std::fs::create_dir_all(outside.join("dir")).unwrap();
    use std::os::unix::fs::symlink;
    symlink(&outside, home.join("link_out")).unwrap();
    symlink(outside.join("secret.txt"), home.join("file_link")).unwrap();
    symlink(home.join("sub"), home.join("link_in")).unwrap();
    symlink(home.join("nowhere"), home.join("dangling")).unwrap();
    symlink("/etc", home.join("link_etc")).unwrap();
    symlink("../../outside", home.join("rel_out")).unwrap();
    let (h, sh) = (home.clone(), shared.clone());
    let read: Rule = Arc::new(move |p: &Path| p.starts_with(&h) || p.starts_with(&sh));
    let h2 = home.clone();
    let write: Rule = Arc::new(move |p: &Path| p.starts_with(&h2));
    let jail = Jail::new(home.clone(), vec![shared.clone()], read, write);
    (World { root, home, shared, outside }, jail)
}
