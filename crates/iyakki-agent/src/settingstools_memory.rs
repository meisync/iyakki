//! Chat tools for Settings -> Memory (the experimental compressed-memory switch): `compressed_memory_status` (free) and
//! `compressed_memory_set` (asks every time, administrators). They read and write the same `compressed-memory` key of `system.conf` as the
//! page's Save button; the setting is used from the next start (the page's Apply now reconfigures a running system). The other memory
//! choices (M37) are `memory_settings_status` (free) and `memory_settings_set` (asks every time, administrators): zram off, auto or a
//! size, the compressor, swappiness, the swap file and zswap, written with the page's own `save_conf`; a choice a boot option fixes is
//! refused, and nothing is applied to the running system (that is the page's Apply now).

use serde_json::{json, Value};

use iyakki_zram::compressed::{self, Experiment};

use crate::settingstools::{arg_bool, no_args, Entry, Env};

const TAB: &str = "Memory";

pub const TOOLS: [&str; 6] = ["compressed_memory_status", "compressed_memory_set", "memory_settings_status", "memory_settings_set", "boot_speed_status", "boot_speed_set"];

pub const PROMPT_RULE: &str = "\nMEMORY (Settings -> Memory): compressed_memory_status shows the experimental compressed-memory switch (free). compressed_memory_set turns it on or off and asks first (administrators only); it is off by default, labelled experimental, and used from the next start: on uses zswap with a small swap file when the data disk has room, zram only otherwise. memory_settings_status shows the zram, compressor, swappiness, swap file and zswap choices and which of them a boot option fixes (free). memory_settings_set changes them (administrators, asks first; used from the next start, a choice fixed by a boot option is refused); zram is off, auto, or a size like 4G or 50%, swap_file is off or a size like 2G. boot_speed_status shows the two developer switches that record which files a start opens and read them ahead into memory (both off by default, not measured to help); boot_speed_set changes them (administrators, asks first, used from the next start).";

fn state(env: &Env, cmdline: &str) -> (Experiment, bool) {
    compressed::resolve_experiment(cmdline, &std::fs::read_to_string(env.dir.join("system.conf")).unwrap_or_default())
}

fn describe(e: Experiment, locked: bool) -> String {
    format!(
        "Experimental compressed memory: {}{}. Off is the default (zram only, as always). On uses zswap with a small swap file when the data disk has room and zram otherwise, and lets the memory gate count half of what the compression saves. Used from the next start.",
        e.id(),
        if locked { " (fixed by a boot option; it cannot be changed here)" } else { "" }
    )
}

fn size_words(s: iyakki_zram::config::Size) -> String {
    use iyakki_zram::config::Size;
    match s {
        Size::Auto => "auto".into(),
        Size::Percent(p) => format!("{p}%"),
        Size::Bytes(b) => format!("{:.1} GiB", b as f64 / (1u64 << 30) as f64),
    }
}

/// The memory choices in force, in words, with the ones a boot option fixes named.
fn memory_text(env: &Env, cmdline: &str) -> String {
    use iyakki_zram::config::ZramMode;
    let conf = std::fs::read_to_string(env.dir.join("system.conf")).unwrap_or_default();
    let r = iyakki_zram::config::resolve(cmdline, &conf);
    let c = &r.config;
    let lock = |b: bool| if b { " (fixed by a boot option)" } else { "" };
    format!(
        "Compressed RAM (zram): {}{}. Compressor: {}. Swappiness: {}{}. Swap file on the data volume: {}{}. zswap: {}{}. Used from the next start; the Memory page's Apply now changes a running system.",
        if c.zram == ZramMode::Off { "off".to_string() } else { format!("on, size {}", size_words(c.size)) },
        lock(r.zram_locked),
        c.algo.as_deref().unwrap_or("zstd (or the best the kernel has)"),
        c.swappiness,
        lock(r.swappiness_locked),
        c.swapfile.map_or("off".to_string(), |b| format!("{:.1} GiB", b as f64 / (1u64 << 30) as f64)),
        lock(r.swapfile_locked),
        if c.zswap { "on" } else { "off" },
        lock(r.zswap_locked),
    )
}

/// The new configuration the arguments ask for, or the sentence that says why not. Nothing is written here.
fn wanted(env: &Env, cmdline: &str, a: &Value) -> Result<iyakki_zram::Config, String> {
    use iyakki_zram::config::{parse_bytes, parse_zram_value};
    let conf = std::fs::read_to_string(env.dir.join("system.conf")).unwrap_or_default();
    let r = iyakki_zram::config::resolve(cmdline, &conf);
    // the file's own values are what gets changed and written; the command line only wins at boot
    let mut c = iyakki_zram::config::resolve("", &conf).config;
    let mut any = false;
    if let Some(v) = a.get("zram") {
        let v = v.as_str().ok_or("zram is a word: off, auto, or a size like 4G or 50%")?;
        let (mode, size, _) = parse_zram_value(v).ok_or("zram is off, auto, or a size like 4G or 50%")?;
        if r.zram_locked {
            return Err("a boot option fixes zram; edit the boot entry to change it".into());
        }
        (c.zram, c.size) = (mode, size);
        any = true;
    }
    if let Some(v) = a.get("compressor") {
        let v = v.as_str().map(str::trim).unwrap_or("").to_ascii_lowercase();
        if v.is_empty() || v.len() > 24 || !v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err("compressor is a name like zstd, lz4 or lzo-rle (auto: the best the kernel has)".into());
        }
        if r.zram_locked {
            return Err("a boot option fixes zram and its compressor; edit the boot entry to change it".into());
        }
        c.algo = (v != "auto").then_some(v);
        any = true;
    }
    if let Some(v) = a.get("swappiness") {
        let n = v.as_u64().filter(|n| *n <= 200).ok_or("swappiness is a whole number from 0 to 200")?;
        if r.swappiness_locked {
            return Err("a boot option fixes swappiness; edit the boot entry to change it".into());
        }
        c.swappiness = n as u32;
        any = true;
    }
    if let Some(v) = a.get("swap_file") {
        let v = v.as_str().ok_or("swap_file is off or a size like 2G")?.trim();
        if r.swapfile_locked {
            return Err("a boot option fixes the swap file; edit the boot entry to change it".into());
        }
        c.swapfile = if matches!(v.to_ascii_lowercase().as_str(), "off" | "0" | "none") { None } else { Some(parse_bytes(v).filter(|b| *b >= 64 << 20).ok_or("swap_file is off or a size of at least 64M, like 2G")?) };
        any = true;
    }
    if let Some(v) = a.get("zswap") {
        let b = v.as_bool().ok_or("zswap is true or false")?;
        if r.zswap_locked {
            return Err("a boot option fixes zswap; edit the boot entry to change it".into());
        }
        c.zswap = b;
        any = true;
    }
    if !any {
        return Err("say what to change: zram, compressor, swappiness, swap_file or zswap".into());
    }
    Ok(c)
}

fn boot_speed_text(env: &Env, cmdline: &str) -> String {
    let b = iyakki_zram::bootspeed::resolve(cmdline, &std::fs::read_to_string(env.dir.join("system.conf")).unwrap_or_default());
    let one = |on: bool, locked: bool| format!("{}{}", if on { "on" } else { "off" }, if locked { " (fixed by a boot option)" } else { "" });
    format!(
        "Record the boot order: {}. Read the recorded boot files ahead into memory: {}. Both are developer options and off by default; reading ahead is not measured to make a start faster. Used from the next start.",
        one(b.record, b.record_locked),
        one(b.prefetch, b.prefetch_locked)
    )
}

fn boot_speed_wanted(env: &Env, cmdline: &str, a: &Value) -> Result<(Option<bool>, Option<bool>), String> {
    let b = iyakki_zram::bootspeed::resolve(cmdline, &std::fs::read_to_string(env.dir.join("system.conf")).unwrap_or_default());
    let get = |k: &str, locked: bool, opt: &str| -> Result<Option<bool>, String> {
        match a.get(k) {
            None => Ok(None),
            Some(v) => {
                let v = v.as_bool().ok_or_else(|| format!("{k} is true or false"))?;
                if locked {
                    return Err(format!("the boot option {opt} fixes this; edit the boot entry to change it"));
                }
                Ok(Some(v))
            }
        }
    };
    let (rec, pre) = (get("record", b.record_locked, "iyakki.bootrecord")?, get("prefetch", b.prefetch_locked, "iyakki.prefetch")?);
    if rec.is_none() && pre.is_none() {
        return Err("say `record` and/or `prefetch`: true or false".into());
    }
    Ok((rec, pre))
}

pub fn entries_with(env: &Env, cmdline: String) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let (e, c) = (env.clone(), cmdline.clone());
        v.push(Entry::read(TAB, "boot_speed_status", "Show the developer switches that record the boot order and read the boot files ahead (off by default).", no_args(), move |_| Ok(boot_speed_text(&e, &c))));
    }
    {
        let (e, e2, e3, c, c2, c3, c4) = (env.clone(), env.clone(), env.clone(), cmdline.clone(), cmdline.clone(), cmdline.clone(), cmdline.clone());
        let schema = json!({"type": "object", "properties": {"record": {"type": "boolean", "description": "Record which files the first 90 seconds of a start open."}, "prefetch": {"type": "boolean", "description": "Read the recorded files into memory in the background after the start."}}});
        v.push(
            Entry::change(TAB, "boot_speed_set", "Switch the boot-order recording and the read-ahead of the boot files on or off, from the next start (developer options, off by default). The person approves a card first.", schema, |_| "Change the boot-speed developer options".into(), move |a| {
                let (rec, pre) = boot_speed_wanted(&e, &c, a)?;
                let path = e.dir.join("system.conf");
                if let Some(r) = rec {
                    iyakki_zram::set_conf_key(&path, iyakki_zram::bootspeed::KEY_RECORD, Some(iyakki_zram::bootspeed::conf_value(r)))?;
                }
                if let Some(p) = pre {
                    iyakki_zram::set_conf_key(&path, iyakki_zram::bootspeed::KEY_PREFETCH, Some(iyakki_zram::bootspeed::conf_value(p)))?;
                }
                e.audit("memory.save", &format!("boot-record={rec:?} boot-prefetch={pre:?}"));
                Ok(format!("Saved. {}", boot_speed_text(&e, &c4)))
            })
            .admin()
            .with_preview(move |a| match boot_speed_wanted(&e2, &c2, a) {
                Ok((r, p)) => format!("Now: {}\nAfter: record {}, prefetch {}", boot_speed_text(&e2, &c2), r.map_or("unchanged", |b| if b { "on" } else { "off" }), p.map_or("unchanged", |b| if b { "on" } else { "off" })),
                Err(e) => format!("Cannot: {e}"),
            })
            .with_refusal(move |a| boot_speed_wanted(&e3, &c3, a).err()),
        );
    }
    {
        let (e, c) = (env.clone(), cmdline.clone());
        v.push(Entry::read(TAB, "memory_settings_status", "Show the memory choices: zram, compressor, swappiness, swap file, zswap.", no_args(), move |_| Ok(memory_text(&e, &c))));
    }
    {
        let (e, e2, e3, c, c2, c3, c4) = (env.clone(), env.clone(), env.clone(), cmdline.clone(), cmdline.clone(), cmdline.clone(), cmdline.clone());
        let schema = json!({"type": "object", "properties": {
            "zram": {"type": "string", "description": "off, auto, or a size like 4G or 50%."},
            "compressor": {"type": "string", "description": "zstd, lz4, lzo-rle ... or auto."},
            "swappiness": {"type": "integer", "description": "0 to 200."},
            "swap_file": {"type": "string", "description": "off, or a size like 2G, on the data volume."},
            "zswap": {"type": "boolean"}
        }});
        v.push(
            Entry::change(TAB, "memory_settings_set", "Change the memory choices (zram size, compressor, swappiness, swap file, zswap), used from the next start. The person approves a card first.", schema, |_| "Change the memory settings".into(), move |a| {
                let cfg = wanted(&e, &c, a)?;
                iyakki_zram::save_conf(&e.dir.join("system.conf"), &cfg)?;
                e.audit("memory.save", &format!("zram={:?} swappiness={} zswap={}", cfg.zram, cfg.swappiness, cfg.zswap));
                Ok(format!("Saved. {}", memory_text(&e, &c4)))
            })
            .admin()
            .with_preview(move |a| match wanted(&e2, &c2, a) {
                Ok(c) => format!("Now: {}\nAfter: zram {:?} {}, compressor {}, swappiness {}, swap file {}, zswap {}", memory_text(&e2, &c2), c.zram, size_words(c.size), c.algo.as_deref().unwrap_or("auto"), c.swappiness, c.swapfile.map_or("off".to_string(), |b| format!("{} MiB", b >> 20)), if c.zswap { "on" } else { "off" }),
                Err(e) => format!("Cannot: {e}"),
            })
            .with_refusal(move |a| wanted(&e3, &c3, a).err()),
        );
    }
    {
        let (e, c) = (env.clone(), cmdline.clone());
        v.push(Entry::read(TAB, "compressed_memory_status", "Show the experimental compressed-memory switch (off by default).", no_args(), move |_| {
            let (x, locked) = state(&e, &c);
            Ok(describe(x, locked))
        }));
    }
    {
        let (e, e2, e3, c, c2, c3) = (env.clone(), env.clone(), env.clone(), cmdline.clone(), cmdline.clone(), cmdline);
        let schema = json!({"type": "object", "required": ["enabled"], "properties": {"enabled": {"type": "boolean", "description": "true: on (experimental); false: off."}}});
        let refuse = move |e: &Env, c: &str, a: &Value| -> Option<String> {
            if arg_bool(a, "enabled").is_err() {
                return Some("say `enabled`: true or false".into());
            }
            state(e, c).1.then(|| "a boot option fixes this setting; edit the boot entry to change it".into())
        };
        v.push(
            Entry::change(TAB, "compressed_memory_set", "Turn the experimental compressed memory on or off, from the next start. The person approves a card first.", schema, |a| format!("Turn experimental compressed memory {}", if a.get("enabled").and_then(Value::as_bool).unwrap_or(false) { "on" } else { "off" }), move |a| {
                if let Some(m) = refuse(&e, &c, a) {
                    return Err(m);
                }
                let x = if arg_bool(a, "enabled")? { Experiment::On } else { Experiment::Off };
                iyakki_zram::set_conf_key(&e.dir.join("system.conf"), "compressed-memory", Some(compressed::conf_value(x)))?;
                e.audit("memory.save", &format!("compressed-memory={}", x.id()));
                Ok(format!("Saved. {}", describe(x, false)))
            })
            .admin()
            .with_preview(move |a| {
                let now = state(&e2, &c2).0;
                format!("Now: {}\nAfter: {}", now.id(), if a.get("enabled").and_then(Value::as_bool).unwrap_or(false) { "on" } else { "off" })
            })
            .with_refusal(move |a| {
                let (x, locked) = state(&e3, &c3);
                let _ = x;
                if arg_bool(a, "enabled").is_err() {
                    return Some("say `enabled`: true or false".into());
                }
                locked.then(|| "a boot option fixes this setting; edit the boot entry to change it".to_string())
            }),
        );
    }
    v
}

pub fn entries(env: &Env) -> Vec<Entry> {
    entries_with(env, crate::sysinfo::cmdline(crate::sysinfo::proc_root()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Tools;
    use crate::settingstools_tests::{assert_policy, rig, tools};

    #[test]
    fn the_switch_is_read_free_changed_by_administrators_with_a_card_and_fixed_by_a_boot_option() {
        let r = rig("memory", true);
        let t = tools(&r, entries_with(&r.env, "quiet".into()));
        assert_policy(&t, &["compressed_memory_status"], &["compressed_memory_set"]);
        assert!(t.call("compressed_memory_status", &json!({})).unwrap().contains("compressed memory: off"));
        assert!(t.refusal("compressed_memory_set", &json!({})).is_some(), "needs enabled");
        t.call("compressed_memory_set", &json!({"enabled": true})).unwrap();
        assert!(std::fs::read_to_string(r.dir.join("system.conf")).unwrap().contains("compressed-memory=on"));
        assert!(t.call("compressed_memory_status", &json!({})).unwrap().contains("compressed memory: on"));
        assert!(t.preview("compressed_memory_set", &json!({"enabled": false})).unwrap().contains("After: off"));
        assert!(r.audits.lock().unwrap().iter().any(|l| l.contains("memory.save compressed-memory=on")));
        t.call("compressed_memory_set", &json!({"enabled": false})).unwrap();
        assert!(t.call("compressed_memory_status", &json!({})).unwrap().contains("compressed memory: off"));
        // a boot option fixes it
        let fixed = tools(&r, entries_with(&r.env, "iyakki.zram=exp".into()));
        assert!(fixed.call("compressed_memory_status", &json!({})).unwrap().contains("compressed memory: on (fixed by a boot option"));
        assert!(fixed.refusal("compressed_memory_set", &json!({"enabled": false})).unwrap().contains("boot option"));
        // not offered to people who are not administrators
        let user = rig("memory-user", false);
        assert!(!tools(&user, entries_with(&user.env, String::new())).defs().iter().any(|d| d.spec.name == "compressed_memory_set"));
        let _ = std::fs::remove_dir_all(&r.dir);
        let _ = std::fs::remove_dir_all(&user.dir);
    }

    #[test]
    fn the_boot_speed_switches_are_read_free_changed_by_administrators_with_a_card_and_a_boot_option_fixes_them() {
        let r = rig("memory-boot", true);
        let t = tools(&r, entries_with(&r.env, "quiet".into()));
        assert_policy(&t, &["boot_speed_status"], &["boot_speed_set"]);
        assert!(t.call("boot_speed_status", &json!({})).unwrap().contains("Record the boot order: off. Read the recorded boot files ahead into memory: off."));
        assert!(t.refusal("boot_speed_set", &json!({})).unwrap().contains("say `record`"));
        assert!(t.refusal("boot_speed_set", &json!({"record": "yes"})).is_some());
        assert!(!r.dir.join("system.conf").exists());
        assert!(t.preview("boot_speed_set", &json!({"prefetch": true})).unwrap().contains("prefetch on"));
        let done = t.call("boot_speed_set", &json!({"record": true})).unwrap();
        assert!(done.contains("Record the boot order: on. Read the recorded boot files ahead into memory: off."), "{done}");
        t.call("boot_speed_set", &json!({"prefetch": true})).unwrap();
        let conf = std::fs::read_to_string(r.dir.join("system.conf")).unwrap();
        assert!(conf.contains("boot-record=on") && conf.contains("boot-prefetch=on"), "{conf}");
        assert!(r.audits.lock().unwrap().iter().any(|l| l.contains("memory.save boot-record=Some(true)")));
        // the other memory tools keep this key
        t.call("memory_settings_set", &json!({"swappiness": 80})).unwrap();
        assert!(std::fs::read_to_string(r.dir.join("system.conf")).unwrap().contains("boot-prefetch=on"));
        // a boot option fixes one of them
        let fixed = tools(&r, entries_with(&r.env, "iyakki.prefetch=off".into()));
        assert!(fixed.call("boot_speed_status", &json!({})).unwrap().contains("ahead into memory: off (fixed by a boot option)"));
        assert!(fixed.refusal("boot_speed_set", &json!({"prefetch": true})).unwrap().contains("iyakki.prefetch"));
        assert!(fixed.refusal("boot_speed_set", &json!({"record": false})).is_none());
        let user = rig("memory-boot-user", false);
        assert!(!tools(&user, entries_with(&user.env, String::new())).defs().iter().any(|d| d.spec.name == "boot_speed_set"));
        let _ = std::fs::remove_dir_all(&r.dir);
        let _ = std::fs::remove_dir_all(&user.dir);
    }

    #[test]
    fn the_other_memory_choices_are_read_free_changed_by_administrators_with_a_card_and_a_boot_option_fixes_them() {
        let r = rig("memory-all", true);
        let t = tools(&r, entries_with(&r.env, "quiet".into()));
        assert_policy(&t, &["memory_settings_status"], &["memory_settings_set"]);
        let first = t.call("memory_settings_status", &json!({})).unwrap();
        assert!(first.contains("zram): on, size auto") && first.contains("Swappiness: 100") && first.contains("Swap file on the data volume: off") && first.contains("zswap: off"), "{first}");
        // nothing named, or a value that is not understood: refused before the card, nothing written
        assert!(t.refusal("memory_settings_set", &json!({})).unwrap().contains("say what to change"));
        for bad in [json!({"swappiness": 500}), json!({"swappiness": "high"}), json!({"zram": "lots"}), json!({"swap_file": "10"}), json!({"swap_file": "1M"}), json!({"compressor": "a b"}), json!({"zswap": "yes"})] {
            assert!(t.refusal("memory_settings_set", &bad).is_some(), "{bad}");
            assert!(t.call("memory_settings_set", &bad).is_err(), "{bad}");
        }
        assert!(!r.dir.join("system.conf").exists(), "nothing was written");
        let card = t.preview("memory_settings_set", &json!({"swappiness": 60, "zswap": true})).unwrap();
        assert!(card.contains("Now:") && card.contains("swappiness 60") && card.contains("zswap on"), "{card}");
        let done = t.call("memory_settings_set", &json!({"zram": "4G", "compressor": "LZ4", "swappiness": 60, "swap_file": "2G", "zswap": true})).unwrap();
        assert!(done.starts_with("Saved.") && done.contains("size 4.0 GiB") && done.contains("Compressor: lz4") && done.contains("Swappiness: 60") && done.contains("file on the data volume: 2.0 GiB"), "{done}");
        let conf = std::fs::read_to_string(r.dir.join("system.conf")).unwrap();
        assert!(conf.contains("swappiness=60") && conf.contains("zswap=on"), "{conf}");
        assert!(r.audits.lock().unwrap().iter().any(|l| l.contains("memory.save zram=On swappiness=60 zswap=true")));
        // another key of the file survives, and the experimental switch is its own tool
        t.call("compressed_memory_set", &json!({"enabled": true})).unwrap();
        t.call("memory_settings_set", &json!({"zram": "auto", "swap_file": "off", "compressor": "auto"})).unwrap();
        let conf = std::fs::read_to_string(r.dir.join("system.conf")).unwrap();
        assert!(conf.contains("compressed-memory=on") && conf.contains("swappiness=60"), "{conf}");
        let after = t.call("memory_settings_status", &json!({})).unwrap();
        assert!(after.contains("zram): on, size auto") && after.contains("file on the data volume: off") && after.contains("zstd"), "{after}");
        // a boot option fixes a choice: that one is refused, the others still change
        let fixed = tools(&r, entries_with(&r.env, "iyakki.swappiness=10 iyakki.zram=2G".into()));
        let s = fixed.call("memory_settings_status", &json!({})).unwrap();
        assert!(s.contains("Swappiness: 10 (fixed by a boot option)") && s.contains("size 2.0 GiB") && s.contains("(fixed by a boot option)"), "{s}");
        assert!(fixed.refusal("memory_settings_set", &json!({"swappiness": 30})).unwrap().contains("boot option"));
        assert!(fixed.refusal("memory_settings_set", &json!({"zram": "off"})).unwrap().contains("boot option"));
        assert!(fixed.refusal("memory_settings_set", &json!({"zswap": false})).is_none());
        // not offered to people who are not administrators
        let user = rig("memory-all-user", false);
        assert!(!tools(&user, entries_with(&user.env, String::new())).defs().iter().any(|d| d.spec.name == "memory_settings_set"));
        let _ = std::fs::remove_dir_all(&r.dir);
        let _ = std::fs::remove_dir_all(&user.dir);
    }
}
