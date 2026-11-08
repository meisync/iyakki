//! Memory figures: RAM and swap from `/proc/meminfo`, zram from `/sys/block/zram0`, pressure from `/proc/pressure/memory`.

use crate::apply::Env;
use crate::config::parse_algos;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ZramStats {
    pub device: String,
    pub algorithm: String,
    pub available_algorithms: Vec<String>,
    pub disksize: u64,
    /// Bytes stored (uncompressed), bytes after compression, bytes of RAM used including overhead.
    pub orig: u64,
    pub compressed: u64,
    pub mem_used: u64,
}

impl ZramStats {
    /// Compression ratio (original / compressed), `None` while empty.
    pub fn ratio(&self) -> Option<f64> {
        (self.compressed > 0).then(|| self.orig as f64 / self.compressed as f64)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemStats {
    pub total: u64,
    pub free: u64,
    pub available: u64,
    pub swap_total: u64,
    pub swap_used: u64,
    pub zram: Option<ZramStats>,
    /// Memory pressure "some" averages over 10 / 60 / 300 s, percent.
    pub pressure: Option<[f32; 3]>,
    /// Active swap areas: (path, used bytes, size bytes, priority).
    pub swaps: Vec<(String, u64, u64, i32)>,
}

pub fn parse_meminfo(text: &str) -> (u64, u64, u64, u64, u64) {
    let get = |k: &str| text.lines().find_map(|l| l.strip_prefix(k)?.trim_start_matches(':').split_whitespace().next()?.parse::<u64>().ok()).unwrap_or(0) * 1024;
    let (total, free) = (get("MemTotal"), get("MemFree"));
    let avail = if text.contains("MemAvailable") { get("MemAvailable") } else { free + get("Cached") + get("Buffers") };
    (total, free, avail, get("SwapTotal"), get("SwapFree"))
}

/// `/proc/swaps`: (path, used, size, priority); sizes there are in KiB.
pub fn parse_swaps(text: &str) -> Vec<(String, u64, u64, i32)> {
    text.lines().skip(1).filter_map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        (f.len() >= 5).then(|| Some((f[0].to_string(), f[3].parse::<u64>().ok()? * 1024, f[2].parse::<u64>().ok()? * 1024, f[4].parse::<i32>().ok()?))).flatten()
    }).collect()
}

/// `mm_stat`: orig_data_size compr_data_size mem_used_total ...
pub fn parse_mm_stat(text: &str) -> Option<(u64, u64, u64)> {
    let v: Vec<u64> = text.split_whitespace().take(3).filter_map(|t| t.parse().ok()).collect();
    (v.len() == 3).then(|| (v[0], v[1], v[2]))
}

/// First line of `/proc/pressure/memory`: `some avg10=0.00 avg60=0.00 avg300=0.00 total=0`.
pub fn parse_pressure(text: &str) -> Option<[f32; 3]> {
    let l = text.lines().find(|l| l.starts_with("some"))?;
    let g = |k: &str| l.split_whitespace().find_map(|t| t.strip_prefix(k)?.parse::<f32>().ok());
    Some([g("avg10=")?, g("avg60=")?, g("avg300=")?])
}

fn read(p: &std::path::Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

/// The first zram device that exists (`zram0`, ...).
pub fn zram_device(env: &Env) -> Option<String> {
    let mut v: Vec<String> = std::fs::read_dir(env.sys.join("block")).ok()?.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with("zram")).collect();
    v.sort();
    v.into_iter().next()
}

pub fn zram_stats(env: &Env) -> Option<ZramStats> {
    let dev = zram_device(env)?;
    let d = env.sys.join("block").join(&dev);
    let (all, cur) = parse_algos(&read(&d.join("comp_algorithm")));
    let (orig, compressed, mem_used) = parse_mm_stat(&read(&d.join("mm_stat"))).unwrap_or((0, 0, 0));
    Some(ZramStats { disksize: read(&d.join("disksize")).trim().parse().unwrap_or(0), device: dev, algorithm: cur.unwrap_or_default(), available_algorithms: all, orig, compressed, mem_used })
}

impl MemStats {
    pub fn read(env: &Env) -> MemStats {
        let (total, free, available, swap_total, swap_free) = parse_meminfo(&read(&env.proc.join("meminfo")));
        MemStats {
            total,
            free,
            available,
            swap_total,
            swap_used: swap_total.saturating_sub(swap_free),
            zram: zram_stats(env),
            pressure: parse_pressure(&read(&env.proc.join("pressure/memory"))),
            swaps: parse_swaps(&read(&env.proc.join("swaps"))),
        }
    }

    /// One-line summary for the SYSTEM tile.
    pub fn summary(&self) -> String {
        let g = |b: u64| format!("{:.1}", b as f64 / (1u64 << 30) as f64);
        let mut s = format!("RAM {} GiB, {} GiB available; swap {} of {} GiB used", g(self.total), g(self.available), g(self.swap_used), g(self.swap_total));
        if let Some(z) = &self.zram {
            s.push_str(&format!("; zram {} {}", z.algorithm, z.ratio().map_or("empty".into(), |r| format!("{r:.1}:1"))));
        }
        s
    }
}

impl MemStats {
    /// Is any active swap area a zram device?
    pub fn has_zram_swap(&self) -> bool {
        self.swaps.iter().any(|w| w.0.starts_with("/dev/zram"))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn zram_swap_is_recognised_by_its_device_path() {
        let mut s = super::MemStats::default();
        assert!(!s.has_zram_swap());
        s.swaps.push(("/swapfile".into(), 0, 1, -2));
        assert!(!s.has_zram_swap());
        s.swaps.push(("/dev/zram0".into(), 0, 1, 100));
        assert!(s.has_zram_swap());
    }

    use super::*;

    #[test]
    fn parsers() {
        let m = "MemTotal:       16384000 kB\nMemFree:         1000000 kB\nMemAvailable:    8000000 kB\nSwapTotal:       4194300 kB\nSwapFree:        4000000 kB\n";
        assert_eq!(parse_meminfo(m), (16384000 * 1024, 1000000 * 1024, 8000000 * 1024, 4194300 * 1024, 4000000 * 1024));
        let old = "MemTotal: 100 kB\nMemFree: 10 kB\nBuffers: 5 kB\nCached: 20 kB\n";
        assert_eq!(parse_meminfo(old).2, 35 * 1024, "old kernels without MemAvailable");
        assert_eq!(parse_meminfo("").0, 0);
        let s = "Filename Type Size Used Priority\n/dev/zram0 partition 1048572 1024 100\n/var/lib/iyakki/swapfile file 2097148 0 10\n";
        assert_eq!(parse_swaps(s), vec![("/dev/zram0".into(), 1024 * 1024, 1048572 * 1024, 100), ("/var/lib/iyakki/swapfile".into(), 0, 2097148 * 1024, 10)]);
        assert_eq!(parse_mm_stat("4096000 1024000 1200000 0 1500000 5 0 0 0\n"), Some((4096000, 1024000, 1200000)));
        assert_eq!(parse_mm_stat("junk"), None);
        assert_eq!(parse_pressure("some avg10=1.50 avg60=0.75 avg300=0.10 total=123\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=0\n"), Some([1.5, 0.75, 0.1]));
        assert_eq!(parse_pressure(""), None);
    }

    #[test]
    fn reads_a_fake_tree_and_tolerates_a_missing_one() {
        let root = std::env::temp_dir().join(format!("iyakki-zst-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let env = Env::under(&root);
        assert_eq!(MemStats::read(&env), MemStats::default(), "nothing there: all zero, no panic");
        let z = root.join("sys/block/zram0");
        std::fs::create_dir_all(&z).unwrap();
        std::fs::write(z.join("comp_algorithm"), "lzo lzo-rle lz4 [zstd]\n").unwrap();
        std::fs::write(z.join("disksize"), "1073741824\n").unwrap();
        std::fs::write(z.join("mm_stat"), "4000 1000 2000 0 0 0 0 0 0\n").unwrap();
        std::fs::create_dir_all(root.join("proc/pressure")).unwrap();
        std::fs::write(root.join("proc/pressure/memory"), "some avg10=2.00 avg60=1.00 avg300=0.50 total=1\n").unwrap();
        std::fs::write(root.join("proc/meminfo"), "MemTotal: 2097152 kB\nMemFree: 1 kB\nMemAvailable: 1048576 kB\nSwapTotal: 1048576 kB\nSwapFree: 524288 kB\n").unwrap();
        let s = MemStats::read(&env);
        let zs = s.zram.clone().unwrap();
        assert_eq!((zs.device.as_str(), zs.algorithm.as_str(), zs.disksize, zs.orig, zs.compressed, zs.mem_used), ("zram0", "zstd", 1 << 30, 4000, 1000, 2000));
        assert_eq!(zs.ratio(), Some(4.0));
        assert_eq!((s.total, s.swap_used, s.pressure), (2 << 30, 512 << 20, Some([2.0, 1.0, 0.5])));
        assert!(s.summary().contains("zram zstd 4.0:1"), "{}", s.summary());
        let _ = std::fs::remove_dir_all(&root);
    }
}
