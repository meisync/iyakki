//! What the administrator asked for, from three places in priority order: the kernel command line
//! (`iyakki.zram=off|auto|<size>[,<algo>]`), then `system.conf`, then the defaults.

use std::collections::BTreeMap;

pub const MIB: u64 = 1024 * 1024;
pub const GIB: u64 = 1024 * MIB;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    /// 50 % of RAM (100 % on machines with 2 GiB or less), 256 MiB to 8 GiB.
    Auto,
    Bytes(u64),
    /// Percent of RAM.
    Percent(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZramMode {
    Off,
    On,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub zram: ZramMode,
    pub size: Size,
    /// Requested compressor; `None` = zstd, falling back to lz4 then lzo-rle.
    pub algo: Option<String>,
    pub swappiness: u32,
    /// Swap file on the data volume, in bytes (`None` = off).
    pub swapfile: Option<u64>,
    pub zswap: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config { zram: ZramMode::On, size: Size::Auto, algo: None, swappiness: 100, swapfile: None, zswap: false }
    }
}

/// A resolved configuration and which parts the kernel command line fixed (Settings shows those as locked).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub config: Config,
    pub zram_locked: bool,
    pub swappiness_locked: bool,
    pub swapfile_locked: bool,
    pub zswap_locked: bool,
}

/// "4G", "512M", "1.5GiB", "2048" (MiB is NOT assumed: a bare number is bytes), "50%".
pub fn parse_size(s: &str) -> Option<Size> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(p) = s.strip_suffix('%') {
        let v: u32 = p.trim().parse().ok()?;
        return (1..=400).contains(&v).then_some(Size::Percent(v));
    }
    parse_bytes(s).map(Size::Bytes)
}

pub fn parse_bytes(s: &str) -> Option<u64> {
    let s = s.trim().to_ascii_lowercase();
    let s = s.strip_suffix("ib").or_else(|| s.strip_suffix('b')).unwrap_or(&s);
    let (num, mult) = match s.chars().last()? {
        'k' => (&s[..s.len() - 1], 1024u64),
        'm' => (&s[..s.len() - 1], MIB),
        'g' => (&s[..s.len() - 1], GIB),
        't' => (&s[..s.len() - 1], 1024 * GIB),
        c if c.is_ascii_digit() => (s, 1),
        _ => return None,
    };
    let v: f64 = num.trim().parse().ok()?;
    if !v.is_finite() || v <= 0.0 {
        return None;
    }
    let b = v * mult as f64;
    (b < 1e15).then_some(b as u64)
}

/// `off`, `auto`, `<size>` or `<size>,<algo>` / `auto,<algo>`. `None` = not understood.
pub fn parse_zram_value(v: &str) -> Option<(ZramMode, Size, Option<String>)> {
    let (head, algo) = match v.split_once(',') {
        Some((h, a)) => (h.trim(), Some(a.trim().to_ascii_lowercase()).filter(|a| !a.is_empty() && a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))),
        None => (v.trim(), None),
    };
    match head.to_ascii_lowercase().as_str() {
        "off" | "0" | "no" | "false" | "none" => Some((ZramMode::Off, Size::Auto, None)),
        "auto" | "on" | "" | "1" | "yes" | "true" => Some((ZramMode::On, Size::Auto, algo)),
        other => parse_size(other).map(|s| (ZramMode::On, s, algo)),
    }
}

/// `key=value` lines of `system.conf`; `#` starts a comment. Later lines win. Unknown keys are kept.
pub fn parse_conf(text: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for l in text.lines() {
        let l = l.split('#').next().unwrap_or("").trim();
        if let Some((k, v)) = l.split_once('=') {
            if !k.trim().is_empty() {
                m.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    m
}

pub fn render_conf(m: &BTreeMap<String, String>) -> String {
    let mut s = String::from("# IYAKKI system settings (Settings, System, Memory). Edit freely; one key=value per line.\n");
    for (k, v) in m {
        s.push_str(&format!("{k}={v}\n"));
    }
    s
}

fn flag(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "on" | "1" | "yes" | "true" | "y" => Some(true),
        "off" | "0" | "no" | "false" | "n" => Some(false),
        _ => None,
    }
}

fn cmdline_param<'a>(cmdline: &'a str, key: &str) -> Option<&'a str> {
    cmdline.split_whitespace().find_map(|t| t.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| v))
}

fn apply_conf(c: &mut Config, m: &BTreeMap<String, String>) {
    if let Some((mode, size, algo)) = m.get("zram").and_then(|v| parse_zram_value(v)) {
        (c.zram, c.size) = (mode, size);
        if algo.is_some() {
            c.algo = algo;
        }
    }
    if let Some(a) = m.get("zram.algo").map(|a| a.to_ascii_lowercase()).filter(|a| !a.is_empty() && a != "auto") {
        c.algo = Some(a);
    }
    if let Some(v) = m.get("swappiness").and_then(|v| v.parse::<u32>().ok()).filter(|v| *v <= 200) {
        c.swappiness = v;
    }
    match m.get("swapfile").map(|v| v.as_str()) {
        Some("off") | Some("0") | Some("") => c.swapfile = None,
        Some(v) => {
            if let Some(b) = parse_bytes(v) {
                c.swapfile = Some(b)
            }
        }
        None => {}
    }
    if let Some(z) = m.get("zswap").and_then(|v| flag(v)) {
        c.zswap = z;
    }
}

/// Defaults, then `system.conf`, then the kernel command line.
pub fn resolve(cmdline: &str, conf: &str) -> Resolved {
    let mut config = Config::default();
    apply_conf(&mut config, &parse_conf(conf));
    let mut r = Resolved { config, zram_locked: false, swappiness_locked: false, swapfile_locked: false, zswap_locked: false };
    if let Some((mode, size, algo)) = cmdline_param(cmdline, "iyakki.zram").and_then(parse_zram_value) {
        (r.config.zram, r.config.size) = (mode, size);
        r.config.algo = algo;
        r.zram_locked = true;
    }
    if let Some(v) = cmdline_param(cmdline, "iyakki.swappiness").and_then(|v| v.parse::<u32>().ok()).filter(|v| *v <= 200) {
        r.config.swappiness = v;
        r.swappiness_locked = true;
    }
    if let Some(v) = cmdline_param(cmdline, "iyakki.swapfile") {
        r.config.swapfile = if matches!(v, "off" | "0") { None } else { parse_bytes(v) };
        r.swapfile_locked = true;
    }
    if let Some(z) = cmdline_param(cmdline, "iyakki.zswap").and_then(flag) {
        r.config.zswap = z;
        r.zswap_locked = true;
    }
    r
}

/// The zram device size for `ram` bytes of memory.
pub fn disk_size(size: Size, ram: u64) -> u64 {
    (match size {
        Size::Auto => {
            let want = if ram <= 2 * GIB { ram } else { ram / 2 };
            want.clamp(256 * MIB, 8 * GIB)
        }
        Size::Percent(p) => (ram / 100 * p as u64 + ram % 100 * p as u64 / 100).clamp(64 * MIB, ram.saturating_mul(4).max(64 * MIB)),
        Size::Bytes(b) => b.clamp(64 * MIB, ram.saturating_mul(4).max(64 * MIB)),
    }) / 4096 * 4096
}

/// Compressor to use given the kernel's list ("lzo [lzo-rle] lz4 zstd"): the request if present, else zstd, lz4, lzo-rle, lzo.
pub fn pick_algo(requested: Option<&str>, available: &[String]) -> Option<String> {
    let has = |a: &str| available.iter().any(|x| x == a);
    requested.filter(|r| has(r)).map(String::from).or_else(|| ["zstd", "lz4", "lzo-rle", "lzo"].iter().find(|a| has(a)).map(|a| a.to_string()))
}

/// Algorithms from a `comp_algorithm` file: (all, current).
pub fn parse_algos(text: &str) -> (Vec<String>, Option<String>) {
    let mut all = Vec::new();
    let mut cur = None;
    for t in text.split_whitespace() {
        if let Some(c) = t.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
            cur = Some(c.to_string());
            all.push(c.to_string());
        } else {
            all.push(t.to_string());
        }
    }
    (all, cur)
}

pub fn human(b: u64) -> String {
    if b >= GIB {
        format!("{:.1} GiB", b as f64 / GIB as f64)
    } else {
        format!("{} MiB", b / MIB)
    }
}

impl Config {
    /// The `zram=` value for `system.conf`.
    pub fn zram_value(&self) -> String {
        let head = match (self.zram, self.size) {
            (ZramMode::Off, _) => return "off".into(),
            (_, Size::Auto) => "auto".to_string(),
            (_, Size::Bytes(b)) if b % GIB == 0 => format!("{}G", b / GIB),
            (_, Size::Bytes(b)) => format!("{}M", b.div_ceil(MIB)),
            (_, Size::Percent(p)) => format!("{p}%"),
        };
        match &self.algo {
            Some(a) => format!("{head},{a}"),
            None => head,
        }
    }

    /// Merge into an existing `system.conf` (other keys are kept).
    pub fn into_conf(&self, mut m: BTreeMap<String, String>) -> BTreeMap<String, String> {
        m.insert("zram".into(), self.zram_value());
        m.remove("zram.algo");
        m.insert("swappiness".into(), self.swappiness.to_string());
        m.insert("swapfile".into(), self.swapfile.map_or("off".into(), |b| if b % GIB == 0 { format!("{}G", b / GIB) } else { format!("{}M", b.div_ceil(MIB)) }));
        m.insert("zswap".into(), if self.zswap { "on" } else { "off" }.into());
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_units() {
        assert_eq!(parse_size("4G"), Some(Size::Bytes(4 * GIB)));
        assert_eq!(parse_size("512MiB"), Some(Size::Bytes(512 * MIB)));
        assert_eq!(parse_size("1.5g"), Some(Size::Bytes(GIB + GIB / 2)));
        assert_eq!(parse_size("50%"), Some(Size::Percent(50)));
        assert_eq!(parse_size("1048576"), Some(Size::Bytes(1048576)));
        assert_eq!(parse_size("2T"), Some(Size::Bytes(2048 * GIB)));
        for bad in ["", "abc", "0", "-1G", "0%", "999%", "G", "1x", "nanG"] {
            assert_eq!(parse_size(bad), None, "{bad}");
        }
    }

    #[test]
    fn zram_values() {
        assert_eq!(parse_zram_value("off"), Some((ZramMode::Off, Size::Auto, None)));
        assert_eq!(parse_zram_value("auto"), Some((ZramMode::On, Size::Auto, None)));
        assert_eq!(parse_zram_value("4G,zstd"), Some((ZramMode::On, Size::Bytes(4 * GIB), Some("zstd".into()))));
        assert_eq!(parse_zram_value("50%"), Some((ZramMode::On, Size::Percent(50), None)));
        assert_eq!(parse_zram_value("auto,LZ4"), Some((ZramMode::On, Size::Auto, Some("lz4".into()))));
        assert_eq!(parse_zram_value("4G,bad algo!"), Some((ZramMode::On, Size::Bytes(4 * GIB), None)), "a junk algorithm is ignored");
        assert_eq!(parse_zram_value("potato"), None);
    }

    #[test]
    fn priority_cmdline_then_conf_then_default() {
        let d = resolve("", "");
        assert_eq!(d.config, Config::default());
        assert!(!d.zram_locked);
        let c = resolve("quiet", "zram=2G,lz4\nswappiness=60\nswapfile=1G\nzswap=on\n# comment\nhostname=x");
        assert_eq!((c.config.zram, c.config.size, c.config.algo.as_deref(), c.config.swappiness, c.config.swapfile, c.config.zswap), (ZramMode::On, Size::Bytes(2 * GIB), Some("lz4"), 60, Some(GIB), true));
        assert!(!c.zram_locked && !c.swapfile_locked);
        let k = resolve("iyakki.zram=off iyakki.swappiness=10 iyakki.swapfile=off iyakki.zswap=off", "zram=2G,lz4\nswappiness=60\nswapfile=1G\nzswap=on");
        assert_eq!((k.config.zram, k.config.swappiness, k.config.swapfile, k.config.zswap), (ZramMode::Off, 10, None, false));
        assert!(k.zram_locked && k.swappiness_locked && k.swapfile_locked && k.zswap_locked);
        let k2 = resolve("iyakki.zram=50%", "zram=2G,lz4");
        assert_eq!((k2.config.size, k2.config.algo), (Size::Percent(50), None), "the command line replaces the algorithm too");
        // garbage is ignored, never fatal
        let g = resolve("iyakki.zram=potato iyakki.swappiness=9999", "zram=??\nswappiness=abc\nswapfile=zz\nzswap=maybe");
        assert_eq!(g.config, Config::default());
        assert!(!g.zram_locked && !g.swappiness_locked);
        assert_eq!(resolve("", "zram.algo=lz4\n").config.algo.as_deref(), Some("lz4"));
    }

    #[test]
    fn disk_size_defaults_min_max_and_low_ram() {
        assert_eq!(disk_size(Size::Auto, 16 * GIB), 8 * GIB, "50 % of 16 GiB");
        assert_eq!(disk_size(Size::Auto, 64 * GIB), 8 * GIB, "capped at 8 GiB");
        assert_eq!(disk_size(Size::Auto, 8 * GIB), 4 * GIB);
        assert_eq!(disk_size(Size::Auto, GIB), GIB, "<= 2 GiB RAM: 100 %");
        assert_eq!(disk_size(Size::Auto, 2 * GIB), 2 * GIB);
        assert_eq!(disk_size(Size::Auto, 512 * MIB), 512 * MIB);
        assert_eq!(disk_size(Size::Auto, 128 * MIB), 256 * MIB, "minimum 256 MiB");
        assert_eq!(disk_size(Size::Percent(25), 8 * GIB), 2 * GIB);
        assert_eq!(disk_size(Size::Bytes(100 * GIB), 4 * GIB), 16 * GIB, "never more than 4x RAM");
        assert_eq!(disk_size(Size::Bytes(GIB), 64 * GIB), GIB, "explicit sizes are not capped at 8 GiB");
        assert_eq!(disk_size(Size::Bytes(4097), 4 * GIB), 64 * MIB);
        assert_eq!(disk_size(Size::Bytes(64 * MIB + 5), 4 * GIB) % 4096, 0);
    }

    #[test]
    fn algorithm_fallbacks() {
        let (all, cur) = parse_algos("lzo [lzo-rle] lz4 zstd\n");
        assert_eq!((all.len(), cur.as_deref()), (4, Some("lzo-rle")));
        assert_eq!(pick_algo(None, &all).as_deref(), Some("zstd"));
        assert_eq!(pick_algo(Some("lz4"), &all).as_deref(), Some("lz4"));
        assert_eq!(pick_algo(Some("842"), &all).as_deref(), Some("zstd"), "unknown request falls back");
        let lz4 = vec!["lz4".to_string(), "lzo".to_string()];
        assert_eq!(pick_algo(None, &lz4).as_deref(), Some("lz4"));
        assert_eq!(pick_algo(None, &["lzo".to_string()]).as_deref(), Some("lzo"));
        assert_eq!(pick_algo(None, &[]), None);
    }

    #[test]
    fn conf_roundtrip_keeps_unknown_keys() {
        let mut c = Config { zram: ZramMode::On, size: Size::Bytes(4 * GIB), algo: Some("zstd".into()), swappiness: 80, swapfile: Some(2 * GIB), zswap: true };
        assert_eq!(c.zram_value(), "4G,zstd");
        let text = render_conf(&c.into_conf(parse_conf("hostname=box\nzram.algo=old\n")));
        let r = resolve("", &text);
        assert_eq!(r.config, c);
        assert!(text.contains("hostname=box") && !text.contains("zram.algo"));
        c.zram = ZramMode::Off;
        assert_eq!(c.zram_value(), "off");
        c.zram = ZramMode::On;
        c.size = Size::Percent(30);
        c.algo = None;
        assert_eq!(c.zram_value(), "30%");
        assert_eq!(resolve("", &render_conf(&c.into_conf(Default::default()))).config, c);
    }
}
