//! Read-only block-device and network-interface figures (UI/OS split step 2: the Disks and Net traits).
//! The parsers are pure (text in, values out) so the local implementation and the tests share them; nothing here does IO.

use crate::error::{ApiError, ApiResult};
use serde::{Deserialize, Serialize};

/// One whole disk or partition as `/proc/partitions` lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockDev {
    pub name: String,
    /// Size in bytes (the file counts 1 KiB blocks).
    pub size_bytes: u64,
    pub major: u32,
    pub minor: u32,
}

/// One mounted file system as `/proc/mounts` lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mount {
    pub source: String,
    pub target: String,
    pub fstype: String,
    pub read_only: bool,
}

/// Byte and packet counters of one network interface (`/proc/net/dev`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetCounters {
    pub name: String,
    pub rx_bytes: u64,
    pub rx_packets: u64,
    pub tx_bytes: u64,
    pub tx_packets: u64,
}

/// Read-only disk information.
pub trait Disks: Send + Sync {
    fn block_devices(&self) -> ApiResult<Vec<BlockDev>>;
    fn mounts(&self) -> ApiResult<Vec<Mount>>;
    /// `(free, total)` bytes of the file system that holds `path` (what `df` shows: the space a normal program may still use).
    fn space(&self, path: &str) -> ApiResult<(u64, u64)>;
    /// A failure message of something that reads or writes the data volume. While the drive is not healthy an I/O error becomes the plain
    /// explanation (the drive is gone); healthy: the message is unchanged. A machine that does not know says the message as it is.
    fn reword_failure(&self, msg: &str) -> String {
        msg.to_string()
    }
}

/// Read-only network interface information.
pub trait Net: Send + Sync {
    fn interfaces(&self) -> ApiResult<Vec<NetCounters>>;
    /// `(interface, address)` of every IPv4 address of this computer, the loopback included.
    fn ipv4_addresses(&self) -> ApiResult<Vec<(String, String)>>;
}

/// Parse `/proc/partitions` (`major minor #blocks name`, a header line first). Junk lines are skipped; no device at all is an error.
pub fn parse_partitions(text: &str) -> ApiResult<Vec<BlockDev>> {
    let v: Vec<BlockDev> = text
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let major = it.next()?.parse().ok()?;
            let minor = it.next()?.parse().ok()?;
            let blocks: u64 = it.next()?.parse().ok()?;
            let name = it.next()?.to_string();
            Some(BlockDev { name, size_bytes: blocks.saturating_mul(1024), major, minor })
        })
        .collect();
    if v.is_empty() {
        return Err(ApiError::Io("partitions lists no device".into()));
    }
    Ok(v)
}

/// Parse `/proc/mounts` (`source target fstype options dump pass`); octal escapes such as `\040` in paths are decoded.
pub fn parse_mounts(text: &str) -> Vec<Mount> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 4 {
                return None;
            }
            Some(Mount { source: unescape(f[0]), target: unescape(f[1]), fstype: f[2].to_string(), read_only: f[3].split(',').any(|o| o == "ro") })
        })
        .collect()
}

fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..=i + 3].iter().all(|c| (b'0'..=b'7').contains(c)) {
            let v = (b[i + 1] - b'0') as u32 * 64 + (b[i + 2] - b'0') as u32 * 8 + (b[i + 3] - b'0') as u32;
            out.push(v as u8);
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parse `/proc/net/dev` (two header lines, then `name: rx_bytes rx_packets ... tx_bytes tx_packets ...`).
pub fn parse_net_dev(text: &str) -> ApiResult<Vec<NetCounters>> {
    let v: Vec<NetCounters> = text
        .lines()
        .filter_map(|l| {
            let (name, rest) = l.split_once(':')?;
            let n: Vec<u64> = rest.split_whitespace().map(|t| t.parse().ok()).collect::<Option<Vec<_>>>()?;
            if n.len() < 10 {
                return None;
            }
            Some(NetCounters { name: name.trim().to_string(), rx_bytes: n[0], rx_packets: n[1], tx_bytes: n[8], tx_packets: n[9] })
        })
        .collect();
    if v.is_empty() {
        return Err(ApiError::Io("net/dev lists no interface".into()));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partitions_are_read_in_bytes_and_junk_is_skipped() {
        let t = "major minor  #blocks  name\n\n   8        0  976762584 sda\n   8        1     524288 sda1\nnot a line\n";
        let v = parse_partitions(t).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].name.as_str(), v[0].size_bytes, v[0].major, v[0].minor), ("sda", 976762584 * 1024, 8, 0));
        assert_eq!(v[1].name, "sda1");
        assert_eq!(parse_partitions("major minor #blocks name\n").unwrap_err().code(), "io");
    }

    #[test]
    fn mounts_decode_escapes_and_read_only_flag() {
        let t = "/dev/sda1 /boot/efi vfat rw,relatime 0 0\n/dev/sdb1 /mnt/My\\040Disk ext4 ro,noatime 0 0\nshort\n";
        let m = parse_mounts(t);
        assert_eq!(m.len(), 2);
        assert!(!m[0].read_only);
        assert_eq!((m[1].target.as_str(), m[1].fstype.as_str(), m[1].read_only), ("/mnt/My Disk", "ext4", true));
        assert!(parse_mounts("").is_empty());
        assert_eq!(parse_mounts("a /b\\04 c ro 0 0")[0].target, "/b\\04");
    }

    #[test]
    fn net_dev_gives_rx_and_tx_counters() {
        let t = "Inter-|   Receive                                                |  Transmit\n face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n    lo:    1000      10    0    0    0     0          0         0     1000      10    0    0    0     0       0          0\n  eth0: 5000000    4000    0    0    0     0          0         3  250000    2000    0    0    0     0       0          0\n";
        let v = parse_net_dev(t).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[1], NetCounters { name: "eth0".into(), rx_bytes: 5_000_000, rx_packets: 4000, tx_bytes: 250_000, tx_packets: 2000 });
        assert!(parse_net_dev("garbage").is_err());
    }
}
