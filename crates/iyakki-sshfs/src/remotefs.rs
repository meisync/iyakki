//! The types the remote files clients share: the entry of a listing, the trait one connection implements, and path cleaning.

use std::io::{Read, Write};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub is_link: bool,
    pub size: u64,
    pub modified: i64,
    pub perms: String,
}

/// What one connection can do. Implemented by the SFTP and FTP clients (and by fakes in tests).
pub trait RemoteFs: Send {
    /// The canonical path and the entries (folder names `.` and `..` and names with a slash are already dropped).
    fn list(&mut self, path: &str) -> Result<(String, Vec<Entry>), String>;
    fn mkdir(&mut self, path: &str) -> Result<(), String>;
    fn rename(&mut self, from: &str, to: &str) -> Result<(), String>;
    fn remove_file(&mut self, path: &str) -> Result<(), String>;
    fn remove_dir(&mut self, path: &str) -> Result<(), String>;
    /// The size of a file, None when it is not there.
    fn size(&mut self, path: &str) -> Result<Option<u64>, String>;
    /// Write the file's bytes from `offset` to `out`. `progress` gets the size of each block and returns false to cancel.
    fn download(&mut self, remote: &str, offset: u64, out: &mut dyn Write, progress: &mut dyn FnMut(u64) -> bool) -> Result<(), String>;
    /// Send `src` (already positioned at `offset`) to `remote`, starting at `offset` of the remote file (0: the file is made new).
    fn upload(&mut self, remote: &str, offset: u64, src: &mut dyn Read, progress: &mut dyn FnMut(u64) -> bool) -> Result<(), String>;
}

pub fn clean_path(p: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for s in p.split('/') {
        match s {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    format!("/{}", parts.join("/"))
}
