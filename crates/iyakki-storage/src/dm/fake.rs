//! A recording fake of the kernel for tests: it decodes and checks every ioctl buffer the way the kernel would, keeps the mappings,
//! loop devices and mounts in memory, and models a mounted file system with a folder on disk so lifecycle tests can write files.
//!
//! The file system model: each mapping owns a store folder `<store>/<mapping>`; mounting moves that folder's content in view at the
//! target (a rename), unmounting moves it back. So files are visible only while mounted, and they stay with the mapping, which is
//! what the real thing does for the person.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::*;

#[derive(Default)]
struct State {
    /// mapping name -> (the decoded table, loop device it sits on)
    maps: BTreeMap<String, Table>,
    loops: BTreeMap<String, PathBuf>,
    next_loop: u32,
    /// target -> mapping device path
    mounts: BTreeMap<PathBuf, String>,
    formatted: Vec<String>,
    log: Vec<String>,
    tables: Vec<TableLoad>,
    /// names of operations that must fail once ("dm:create", "dm:table", "dm:resume", "dm:remove", "loop", "mkfs", "mount", "umount")
    fail: Vec<String>,
}

#[derive(Clone)]
struct Table {
    load: Option<TableLoad>,
    live: bool,
}

pub struct FakeKernel {
    store: PathBuf,
    st: Mutex<State>,
}

impl FakeKernel {
    /// `store` is a folder (a temp dir in tests) that holds the model file systems.
    pub fn new(store: &Path) -> FakeKernel {
        let _ = std::fs::create_dir_all(store);
        FakeKernel { store: store.to_path_buf(), st: Mutex::new(State::default()) }
    }

    fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Make the next operation of this kind fail (see `State::fail`).
    pub fn fail_once(&self, op: &str) {
        self.st().fail.push(op.into());
    }

    fn should_fail(&self, op: &str) -> bool {
        let mut s = self.st();
        if let Some(i) = s.fail.iter().position(|f| f == op) {
            s.fail.remove(i);
            return true;
        }
        false
    }

    /// Every operation in order, for example `dm:create iyakki-home-amy`. Never holds a key.
    pub fn log(&self) -> Vec<String> {
        self.st().log.clone()
    }

    /// Every table that was loaded (these DO hold the key: the tests compare the line).
    pub fn tables(&self) -> Vec<TableLoad> {
        self.st().tables.clone()
    }

    pub fn mapping_names(&self) -> Vec<String> {
        self.st().maps.keys().cloned().collect()
    }

    pub fn loop_devices(&self) -> Vec<String> {
        self.st().loops.keys().cloned().collect()
    }

    pub fn mounted_at(&self, target: &Path) -> bool {
        self.st().mounts.contains_key(target)
    }

    /// The power goes: every mount disappears (the model file systems keep their content, as a disk does), mappings and loop devices are gone.
    pub fn power_cut(&self) {
        let mut s = self.st();
        let mounts: Vec<(PathBuf, String)> = s.mounts.iter().map(|(t, d)| (t.clone(), d.clone())).collect();
        for (target, dev) in mounts {
            let name = dev.strip_prefix("/dev/mapper/").unwrap_or("").to_string();
            let shadow = PathBuf::from(format!("{}.shadow", target.display()));
            let _ = std::fs::rename(&target, self.fs_dir(&name));
            let _ = std::fs::rename(&shadow, &target);
        }
        s.mounts.clear();
        s.maps.clear();
        s.loops.clear();
        s.log.push("power cut".into());
    }

    fn fs_dir(&self, map: &str) -> PathBuf {
        self.store.join(map)
    }
}

fn err(msg: &str, errno: i32) -> io::Error {
    let _ = msg;
    io::Error::from_raw_os_error(errno)
}

impl DmKernel for FakeKernel {
    fn dm_ioctl(&self, cmd: DmCmd, buf: &mut Vec<u8>) -> io::Result<()> {
        if buf.len() < DM_IOCTL_LEN || get_u32(buf, 0) != 4 {
            return Err(err("bad header", libc::EINVAL));
        }
        let name = header_name(buf).unwrap_or_default();
        let op = match cmd {
            DmCmd::Version => "version",
            DmCmd::DevCreate => "create",
            DmCmd::DevRemove => "remove",
            DmCmd::DevSuspend => "resume",
            DmCmd::DevStatus => "status",
            DmCmd::TableLoad => "table",
        };
        if self.should_fail(&format!("dm:{op}")) {
            return Err(err("injected", libc::EIO));
        }
        let mut s = self.st();
        s.log.push(format!("dm:{op} {name}"));
        match cmd {
            DmCmd::Version => Ok(()),
            DmCmd::DevCreate => {
                if !valid_name(&name) {
                    return Err(err("name", libc::EINVAL));
                }
                if s.maps.contains_key(&name) {
                    return Err(err("exists", libc::EBUSY));
                }
                s.maps.insert(name, Table { load: None, live: false });
                Ok(())
            }
            DmCmd::TableLoad => {
                let t = decode_table_load(buf).map_err(|e| {
                    s.log.push(format!("dm:table rejected: {e}"));
                    err(e, libc::EINVAL)
                })?;
                if t.target_type != "crypt" {
                    return Err(err("target", libc::EINVAL));
                }
                if t.flags & DM_SECURE_DATA_FLAG == 0 {
                    return Err(err("the key buffer must be flagged secure", libc::EINVAL));
                }
                // the kernel's own parse of the parameter string: cipher key iv_offset device offset [n opts...]
                let f: Vec<&str> = t.params.split(' ').collect();
                let ok = f.len() >= 5 && f[0] == CIPHER && f[1].len() == KEY_LEN * 2 && f[1].bytes().all(|b| b.is_ascii_hexdigit()) && f[2] == "0" && f[3].starts_with("/dev/loop") && f[4].parse::<u64>().is_ok();
                if !ok {
                    return Err(err("crypt parameters", libc::EINVAL));
                }
                if !s.loops.contains_key(f[3]) {
                    return Err(err("no such device", libc::ENODEV));
                }
                let m = s.maps.get_mut(&name).ok_or_else(|| err("no mapping", libc::ENXIO))?;
                m.load = Some(t.clone());
                s.tables.push(t);
                Ok(())
            }
            DmCmd::DevSuspend => {
                let m = s.maps.get_mut(&name).ok_or_else(|| err("no mapping", libc::ENXIO))?;
                if header_flags(buf) & DM_SUSPEND_FLAG == 0 {
                    if m.load.is_none() {
                        return Err(err("no table", libc::ENXIO));
                    }
                    m.live = true;
                }
                Ok(())
            }
            DmCmd::DevStatus => {
                let m = s.maps.get(&name).ok_or_else(|| err("no mapping", libc::ENXIO))?;
                let open = s.mounts.values().filter(|d| **d == map_path(&name)).count() as u32;
                let flags = if m.live { DM_ACTIVE_PRESENT_FLAG } else { 0 };
                put_u32(buf, 24, open);
                put_u32(buf, 28, flags);
                Ok(())
            }
            DmCmd::DevRemove => {
                if !s.maps.contains_key(&name) {
                    return Err(err("no mapping", libc::ENXIO));
                }
                if s.mounts.values().any(|d| *d == map_path(&name)) {
                    return Err(err("busy", libc::EBUSY));
                }
                s.maps.remove(&name);
                Ok(())
            }
        }
    }

    fn loop_attach(&self, file: &Path) -> io::Result<String> {
        if self.should_fail("loop") {
            return Err(err("injected", libc::EIO));
        }
        if !file.is_file() {
            return Err(err("no file", libc::ENOENT));
        }
        let mut s = self.st();
        let dev = format!("/dev/loop{}", s.next_loop);
        s.next_loop += 1;
        s.loops.insert(dev.clone(), file.to_path_buf());
        s.log.push(format!("loop:attach {dev}"));
        Ok(dev)
    }

    fn loop_detach(&self, dev: &str) -> io::Result<()> {
        let mut s = self.st();
        if s.maps.values().any(|m| m.load.as_ref().is_some_and(|t| t.params.split(' ').nth(3) == Some(dev))) {
            return Err(err("busy", libc::EBUSY));
        }
        s.loops.remove(dev).ok_or_else(|| err("no loop device", libc::ENXIO))?;
        s.log.push(format!("loop:detach {dev}"));
        Ok(())
    }

    fn make_fs(&self, dev: &str, label: &str) -> io::Result<()> {
        if self.should_fail("mkfs") {
            return Err(err("injected", libc::EIO));
        }
        let name = dev.strip_prefix("/dev/mapper/").unwrap_or("").to_string();
        let mut s = self.st();
        if !s.maps.get(&name).is_some_and(|m| m.live) {
            return Err(err("no such device", libc::ENODEV));
        }
        std::fs::create_dir_all(self.fs_dir(&name))?;
        s.formatted.push(name.clone());
        s.log.push(format!("mkfs {name} label={label}"));
        Ok(())
    }

    fn mount(&self, dev: &str, target: &Path) -> io::Result<()> {
        if self.should_fail("mount") {
            return Err(err("injected", libc::EIO));
        }
        let name = dev.strip_prefix("/dev/mapper/").unwrap_or("").to_string();
        let mut s = self.st();
        if !s.maps.get(&name).is_some_and(|m| m.live) {
            return Err(err("no such device", libc::ENODEV));
        }
        if !s.formatted.contains(&name) && !self.fs_dir(&name).is_dir() {
            return Err(err("no file system", libc::EINVAL));
        }
        if s.mounts.contains_key(target) {
            return Err(err("busy", libc::EBUSY));
        }
        if !target.is_dir() {
            return Err(err("no mount point", libc::ENOENT));
        }
        // the content of the model file system comes into view; whatever was in the folder before is shadowed (moved aside)
        let shadow = PathBuf::from(format!("{}.shadow", target.display()));
        std::fs::rename(target, &shadow)?;
        std::fs::rename(self.fs_dir(&name), target)?;
        s.mounts.insert(target.to_path_buf(), dev.to_string());
        s.log.push(format!("mount {name} {}", target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
        Ok(())
    }

    fn umount(&self, target: &Path) -> io::Result<()> {
        if self.should_fail("umount") {
            return Err(err("injected", libc::EBUSY));
        }
        let mut s = self.st();
        let dev = s.mounts.get(target).cloned().ok_or_else(|| err("not mounted", libc::EINVAL))?;
        let name = dev.strip_prefix("/dev/mapper/").unwrap_or("").to_string();
        let shadow = PathBuf::from(format!("{}.shadow", target.display()));
        std::fs::rename(target, self.fs_dir(&name))?;
        std::fs::rename(&shadow, target)?;
        s.mounts.remove(target);
        s.log.push(format!("umount {name}"));
        Ok(())
    }

    fn is_mounted(&self, target: &Path) -> bool {
        self.st().mounts.contains_key(target)
    }
}
