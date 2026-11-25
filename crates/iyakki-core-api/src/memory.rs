//! Compressed memory (zram), swap and the `system.conf` that holds their settings, as the core serves them (UI/OS split plan, F-ARCH-05/06).
//! The data types and the pure rules (parsing a size, resolving the boot option and the file, planning, previewing) are those of `iyakki-zram`
//! and are re-exported here, so a tile names them through the core interface. Everything that reads the kernel's figures, writes the file,
//! turns swap on or off or asks the disk for room goes through [`Memory`]: the device has `iyakki_core_local::LocalMemory`, a test has the fake.

use std::path::{Path, PathBuf};

pub use iyakki_zram::apply::{swapoff_safe, Step, Swapper};
pub use iyakki_zram::{bootspeed, compressed, config, preview, stats, MemStats};
pub use iyakki_zram::{CONF_PATH, SWAPFILE_PATH};

use config::Config;

/// What the data folder lives on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataPlace {
    /// A disk volume: data survives power-off.
    Disk,
    /// RAM (a live session): data is lost at power-off.
    Memory,
    /// Something else (a container volume, a network share).
    Other,
}

/// `2026-10-23 16:40:01` (UTC) for a Unix time.
pub use iyakki_storage::crashlog::wall;

/// What the Memory page does to the machine. Errors are sentences for the person.
pub trait Memory: Send + Sync {
    /// The kernel's memory, swap and zram figures now.
    fn stats(&self) -> MemStats;
    /// Where `system.conf` is.
    fn conf_path(&self) -> PathBuf;
    /// The text of `system.conf` (empty when there is none).
    fn read_conf(&self) -> String;
    /// Write the memory settings into `system.conf`, keeping the keys that are not ours.
    fn save_conf(&self, cfg: &Config) -> Result<(), String>;
    /// Set (`Some`) or remove (`None`) one key of `system.conf`.
    fn set_conf_key(&self, key: &str, value: Option<&str>) -> Result<(), String>;
    /// Apply the settings to the running system: zram is taken out of use, reset and set up again.
    fn reconfigure(&self, cfg: &Config, ram: u64) -> Result<Vec<Step>, String>;
    /// Turn the swap file on or off (`bytes`), when the data disk has room (`on_disk`).
    fn apply_swapfile(&self, bytes: Option<u64>, on_disk: bool) -> Vec<Step>;
    /// Where the data folder lives. The compressed-memory plan and the swap file depend on it, and so does the warning that data is kept in memory only.
    fn data_place(&self, dir: &Path) -> DataPlace;
    fn data_on_disk(&self, dir: &Path) -> bool {
        self.data_place(dir) == DataPlace::Disk
    }
    /// Free bytes where the data lives, for the experimental compressed-memory plan; `None` when unknown.
    fn data_free(&self, dir: &Path) -> Option<u64>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// A machine whose only fact is where the data folder lives; every other call answers with nothing.
    struct Place(DataPlace);

    impl Memory for Place {
        fn stats(&self) -> MemStats {
            MemStats::default()
        }
        fn conf_path(&self) -> PathBuf {
            PathBuf::new()
        }
        fn read_conf(&self) -> String {
            String::new()
        }
        fn save_conf(&self, _cfg: &config::Config) -> Result<(), String> {
            Ok(())
        }
        fn set_conf_key(&self, _key: &str, _value: Option<&str>) -> Result<(), String> {
            Ok(())
        }
        fn reconfigure(&self, _cfg: &config::Config, _ram: u64) -> Result<Vec<Step>, String> {
            Ok(Vec::new())
        }
        fn apply_swapfile(&self, _bytes: Option<u64>, _on_disk: bool) -> Vec<Step> {
            Vec::new()
        }
        fn data_place(&self, _dir: &Path) -> DataPlace {
            self.0
        }
        fn data_free(&self, _dir: &Path) -> Option<u64> {
            None
        }
    }

    #[test]
    fn data_on_a_disk_volume_survives_power_off_so_it_counts_as_on_disk() {
        assert!(Place(DataPlace::Disk).data_on_disk(Path::new("/var/lib/iyakki")));
    }

    #[test]
    fn data_held_in_ram_or_on_another_kind_of_volume_does_not_count_as_on_disk() {
        assert!(!Place(DataPlace::Memory).data_on_disk(Path::new("/var/lib/iyakki")), "a live session loses its data at power-off");
        assert!(!Place(DataPlace::Other).data_on_disk(Path::new("/var/lib/iyakki")), "a container volume or share is not a disk volume");
    }

    #[test]
    fn the_data_place_is_passed_through_unchanged_for_every_folder() {
        let p = Place(DataPlace::Memory);
        assert_eq!(p.data_place(Path::new("/a")), DataPlace::Memory);
        assert_eq!(p.data_place(Path::new("/b")), DataPlace::Memory);
    }
}
