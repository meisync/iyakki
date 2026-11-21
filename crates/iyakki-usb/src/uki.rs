//! A unified kernel image builder (F-BOOT-02, "signed unified kernel image"): one EFI program that carries the kernel, the initial RAM disk
//! and the command line as PE sections, so ONE signature covers all three (with GRUB the initial RAM disk and the command line are read
//! from the disk unsigned). It appends the sections `.osrel`, `.cmdline`, `.initrd` and `.linux` (the UAPI "UKI" layout) to an unsigned EFI
//! stub, in pure Rust: no `objcopy`, no `ukify`, no third-party code. The stub is an input (the section loader that starts `.linux` with
//! `.initrd` and `.cmdline`; our own boot loader or a distribution's); signing the result is `dist/buildroot/secureboot.sh sign-pe`.
//!
//! What it checks: a PE32+ file (x86-64 or aarch64) without a signature (a signed stub would lose it), room in the header for the new
//! section entries, no section of the same name already there, every payload non-empty. What it never does: read a key, run a program or
//! change the stub's own sections.
// SPDX-License-Identifier: Apache-2.0

const SECTION_ENTRY: usize = 40;
/// initialized data, readable
const CHARACTERISTICS: u32 = 0x4000_0040;

/// A section to append: its 8-byte name and its bytes.
pub struct Part<'a> {
    pub name: &'a str,
    pub data: &'a [u8],
}

fn u16at(b: &[u8], o: usize) -> Result<u16, String> {
    b.get(o..o + 2).map(|x| u16::from_le_bytes([x[0], x[1]])).ok_or_else(|| "the file ends inside the PE header".to_string())
}
fn u32at(b: &[u8], o: usize) -> Result<u32, String> {
    b.get(o..o + 4).map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]])).ok_or_else(|| "the file ends inside the PE header".to_string())
}
fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn up(v: u64, a: u64) -> u64 {
    v.div_ceil(a) * a
}

/// Append `parts` to the PE image `stub`, in the order given.
pub fn append_sections(stub: &[u8], parts: &[Part]) -> Result<Vec<u8>, String> {
    if stub.get(..2) != Some(b"MZ") {
        return Err("the stub is not a PE file (no MZ header)".into());
    }
    let pe = u32at(stub, 0x3c)? as usize;
    if stub.get(pe..pe + 4) != Some(b"PE\0\0") {
        return Err("the stub is not a PE file (no PE signature)".into());
    }
    let coff = pe + 4;
    let machine = u16at(stub, coff)?;
    if !matches!(machine, 0x8664 | 0xAA64) {
        return Err(format!("the stub is for machine {machine:#06x}; only x86-64 and aarch64 are supported"));
    }
    let nsec = u16at(stub, coff + 2)? as usize;
    let opt_size = u16at(stub, coff + 16)? as usize;
    let opt = coff + 20;
    if u16at(stub, opt)? != 0x20b {
        return Err("the stub is not a PE32+ (64-bit) image".into());
    }
    let sect_align = u32at(stub, opt + 32)? as u64;
    let file_align = u32at(stub, opt + 36)? as u64;
    let size_of_headers = u32at(stub, opt + 60)? as usize;
    if !sect_align.is_power_of_two() || !file_align.is_power_of_two() || file_align > sect_align {
        return Err("the stub's alignments are not usable".into());
    }
    // the certificate table (data directory 4): a signed stub would be broken by appending
    let ndirs = u32at(stub, opt + 108)? as usize;
    if ndirs > 4 && (u32at(stub, opt + 112 + 4 * 8)? != 0 || u32at(stub, opt + 112 + 4 * 8 + 4)? != 0) {
        return Err("the stub is already signed; give the unsigned stub and sign the result".into());
    }
    let table = opt + opt_size;
    let end_of_table = table + nsec * SECTION_ENTRY;
    let (mut max_va, mut max_raw) = (0u64, 0u64);
    let mut names: Vec<String> = Vec::new();
    for i in 0..nsec {
        let e = table + i * SECTION_ENTRY;
        let name = stub.get(e..e + 8).ok_or("the section table is cut off")?;
        names.push(String::from_utf8_lossy(name).trim_end_matches('\0').to_string());
        let (vsize, va, rsize, raw) = (u32at(stub, e + 8)? as u64, u32at(stub, e + 12)? as u64, u32at(stub, e + 16)? as u64, u32at(stub, e + 20)? as u64);
        max_va = max_va.max(va + vsize.max(rsize));
        max_raw = max_raw.max(raw + rsize);
    }
    for p in parts {
        if p.name.is_empty() || p.name.len() > 8 || !p.name.starts_with('.') || !p.name.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(format!("{:?} is not a section name (a dot and at most 7 more characters)", p.name));
        }
        if p.data.is_empty() {
            return Err(format!("the section {} would be empty", p.name));
        }
        if names.iter().any(|n| n == p.name) || parts.iter().filter(|q| q.name == p.name).count() > 1 {
            return Err(format!("the section {} is there already", p.name));
        }
    }
    // the new entries must fit between the table's end and the first section's data (the headers' room)
    let first_raw = (0..nsec).map(|i| u32at(stub, table + i * SECTION_ENTRY + 20).unwrap_or(u32::MAX) as usize).filter(|r| *r != 0).min().unwrap_or(size_of_headers);
    let room = first_raw.min(size_of_headers.max(end_of_table)).saturating_sub(end_of_table);
    if room < parts.len() * SECTION_ENTRY {
        return Err("there is no room in the stub's header for the new section entries".into());
    }
    let mut out = stub.to_vec();
    out.resize(up(max_raw.max(out.len() as u64), file_align) as usize, 0);
    let mut va = up(max_va, sect_align);
    let mut init_data = u32at(stub, opt + 8)? as u64;
    for (k, p) in parts.iter().enumerate() {
        let raw = out.len() as u64;
        let rsize = up(p.data.len() as u64, file_align);
        out.extend_from_slice(p.data);
        out.resize((raw + rsize) as usize, 0);
        let e = table + (nsec + k) * SECTION_ENTRY;
        out[e..e + SECTION_ENTRY].fill(0);
        out[e..e + p.name.len()].copy_from_slice(p.name.as_bytes());
        put32(&mut out, e + 8, p.data.len() as u32);
        put32(&mut out, e + 12, va as u32);
        put32(&mut out, e + 16, rsize as u32);
        put32(&mut out, e + 20, raw as u32);
        put32(&mut out, e + 36, CHARACTERISTICS);
        va = up(va + p.data.len() as u64, sect_align);
        init_data += rsize;
    }
    out[coff + 2..coff + 4].copy_from_slice(&((nsec + parts.len()) as u16).to_le_bytes());
    put32(&mut out, opt + 8, init_data as u32);
    put32(&mut out, opt + 56, va as u32); // SizeOfImage
    put32(&mut out, opt + 64, 0); // CheckSum: sbsign writes the final one
    Ok(out)
}

/// The four sections of a unified kernel image, in the order the UAPI specification lists them. `osrel` and `cmdline` are text.
pub fn build(stub: &[u8], osrel: &str, cmdline: &str, initrd: &[u8], linux: &[u8]) -> Result<Vec<u8>, String> {
    if cmdline.trim().is_empty() {
        return Err("the command line is empty".into());
    }
    if !osrel.contains("ID=") {
        return Err("the os-release text has no ID= line".into());
    }
    // the command line is one line, NUL-terminated as the stubs expect
    let cl = format!("{}\0", cmdline.trim().replace('\n', " "));
    let osr = format!("{}\0", osrel.trim_end());
    append_sections(stub, &[Part { name: ".osrel", data: osr.as_bytes() }, Part { name: ".cmdline", data: cl.as_bytes() }, Part { name: ".initrd", data: initrd }, Part { name: ".linux", data: linux }])
}

/// The sections of a PE file: (name, virtual address, virtual size, raw offset, raw size).
pub fn sections(pe: &[u8]) -> Result<Vec<(String, u32, u32, u32, u32)>, String> {
    let p = u32at(pe, 0x3c)? as usize;
    let coff = p + 4;
    let n = u16at(pe, coff + 2)? as usize;
    let table = coff + 20 + u16at(pe, coff + 16)? as usize;
    (0..n)
        .map(|i| {
            let e = table + i * SECTION_ENTRY;
            let name = pe.get(e..e + 8).ok_or("the section table is cut off")?;
            Ok((String::from_utf8_lossy(name).trim_end_matches('\0').to_string(), u32at(pe, e + 12)?, u32at(pe, e + 8)?, u32at(pe, e + 20)?, u32at(pe, e + 16)?))
        })
        .collect()
}

/// `iyakki-usb --uki STUB --linux K --initrd I --cmdline FILE|TEXT --osrel FILE --out OUT` (a build tool; `secureboot.sh uki` calls it).
pub fn run_cli(args: &[String]) -> Result<(), String> {
    let mut it = args.iter();
    let (mut stub, mut linux, mut initrd, mut cmd, mut osrel, mut out) = (None, None, None, None, None, None);
    while let Some(a) = it.next() {
        let mut v = |n: &str| it.next().cloned().ok_or(format!("{n} needs a value"));
        match a.as_str() {
            "--uki" => stub = Some(v("--uki")?),
            "--linux" => linux = Some(v("--linux")?),
            "--initrd" => initrd = Some(v("--initrd")?),
            "--cmdline" => cmd = Some(v("--cmdline")?),
            "--osrel" => osrel = Some(v("--osrel")?),
            "--out" => out = Some(v("--out")?),
            other => return Err(format!("unknown option {other}")),
        }
    }
    let read = |p: &Option<String>, what: &str| -> Result<Vec<u8>, String> {
        let p = p.as_ref().ok_or(format!("{what} is needed"))?;
        std::fs::read(p).map_err(|e| format!("{p}: {e}"))
    };
    let cmdline = match cmd.as_ref() {
        Some(c) if std::path::Path::new(c).is_file() => std::fs::read_to_string(c).map_err(|e| e.to_string())?,
        Some(c) => c.clone(),
        None => return Err("--cmdline is needed".into()),
    };
    let img = build(&read(&stub, "--uki STUB")?, &String::from_utf8_lossy(&read(&osrel, "--osrel")?), &cmdline, &read(&initrd, "--initrd")?, &read(&linux, "--linux")?)?;
    let out = out.ok_or("--out is needed")?;
    std::fs::write(&out, img).map_err(|e| format!("{out}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal unsigned PE32+ stub: DOS header, one `.text` section with `text`.
    fn stub(machine: u16, signed: bool, header_room: bool) -> Vec<u8> {
        let (sect_align, file_align) = (0x1000u32, 0x200u32);
        let mut b = vec![0u8; 0x400];
        b[..2].copy_from_slice(b"MZ");
        b[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        let pe = 0x80;
        b[pe..pe + 4].copy_from_slice(b"PE\0\0");
        let coff = pe + 4;
        b[coff..coff + 2].copy_from_slice(&machine.to_le_bytes());
        b[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes());
        let opt_size = 112 + 16 * 8;
        b[coff + 16..coff + 18].copy_from_slice(&(opt_size as u16).to_le_bytes());
        let opt = coff + 20;
        b[opt..opt + 2].copy_from_slice(&0x20bu16.to_le_bytes());
        put32(&mut b, opt + 32, sect_align);
        put32(&mut b, opt + 36, file_align);
        put32(&mut b, opt + 56, 0x2000); // SizeOfImage
        // the headers end where the first section's data starts; a tight header leaves no room
        let first_raw = if header_room { 0x400u32 } else { (opt + opt_size + 40) as u32 };
        put32(&mut b, opt + 60, first_raw);
        put32(&mut b, opt + 108, 16);
        if signed {
            put32(&mut b, opt + 112 + 4 * 8, 0x300);
            put32(&mut b, opt + 112 + 4 * 8 + 4, 0x40);
        }
        let t = opt + opt_size;
        b[t..t + 5].copy_from_slice(b".text");
        put32(&mut b, t + 8, 0x100);
        put32(&mut b, t + 12, 0x1000);
        put32(&mut b, t + 16, 0x200);
        put32(&mut b, t + 20, first_raw.max(0x400));
        if !header_room {
            // a section that starts straight after the table
            put32(&mut b, t + 20, first_raw);
            b.resize(first_raw as usize + 0x200, 0);
        } else {
            b.resize(0x600, 0);
        }
        b
    }

    #[test]
    fn the_four_sections_are_appended_aligned_and_the_header_is_updated() {
        let s = stub(0x8664, false, true);
        let before = sections(&s).unwrap();
        let linux = vec![0xAAu8; 5000];
        let initrd = vec![0xBBu8; 700];
        let img = build(&s, "ID=iyakki\nNAME=IYAKKI\n", "root=/dev/ram0 quiet\n", &initrd, &linux).unwrap();
        let after = sections(&img).unwrap();
        assert_eq!(after.len(), 5);
        assert_eq!(&after[..1], &before[..]);
        let names: Vec<&str> = after.iter().map(|s| s.0.as_str()).collect();
        assert_eq!(names, [".text", ".osrel", ".cmdline", ".initrd", ".linux"]);
        for (n, va, vsize, raw, rsize) in &after[1..] {
            assert_eq!(va % 0x1000, 0, "{n} virtual address");
            assert_eq!((raw % 0x200, rsize % 0x200), (0, 0), "{n} raw alignment");
            assert!(*rsize >= *vsize);
        }
        // addresses climb without overlap, and the bytes are where the table says
        for w in after.windows(2) {
            assert!(w[1].1 >= w[0].1 + w[0].2.max(1), "{w:?}");
        }
        let (_, _, vs, raw, _) = &after[4];
        assert_eq!(*vs as usize, linux.len());
        assert_eq!(&img[*raw as usize..*raw as usize + linux.len()], &linux[..]);
        let (_, _, vs, raw, _) = &after[2];
        assert_eq!(&img[*raw as usize..*raw as usize + *vs as usize], b"root=/dev/ram0 quiet\0");
        // SizeOfImage covers the last section; the stub's own bytes are untouched
        let opt = 0x80 + 4 + 20;
        let last = after.last().unwrap();
        assert!(u32at(&img, opt + 56).unwrap() >= last.1 + last.2);
        assert_eq!(&img[..0x600], &s[..0x600].iter().enumerate().map(|(i, b)| if (0x80 + 4 + 2..0x80 + 4 + 4).contains(&i) || (opt + 8..opt + 12).contains(&i) || (opt + 56..opt + 60).contains(&i) || (0x80 + 4 + 20 + 112 + 128 + 40..0x80 + 4 + 20 + 112 + 128 + 40 * 5).contains(&i) { img[i] } else { *b }).collect::<Vec<u8>>()[..]);
    }

    #[test]
    fn a_signed_stub_a_foreign_machine_a_tight_header_a_clash_and_empty_parts_are_refused() {
        let ok = stub(0x8664, false, true);
        assert!(build(&stub(0x8664, true, true), "ID=x", "a", b"i", b"k").unwrap_err().contains("already signed"));
        assert!(build(&stub(0x014c, false, true), "ID=x", "a", b"i", b"k").unwrap_err().contains("only x86-64 and aarch64"));
        assert!(build(&stub(0x8664, false, false), "ID=x", "a", b"i", b"k").unwrap_err().contains("no room"));
        assert!(build(&ok, "ID=x", "  \n", b"i", b"k").unwrap_err().contains("command line is empty"));
        assert!(build(&ok, "NAME=x", "a", b"i", b"k").unwrap_err().contains("ID="));
        assert!(build(&ok, "ID=x", "a", b"", b"k").unwrap_err().contains(".initrd would be empty"));
        assert!(append_sections(&ok, &[Part { name: ".text", data: b"x" }]).unwrap_err().contains("already"));
        assert!(append_sections(&ok, &[Part { name: "toolongname", data: b"x" }]).unwrap_err().contains("not a section name"));
        assert!(append_sections(b"not a pe", &[]).unwrap_err().contains("MZ"));
        assert!(append_sections(&ok[..0x90], &[Part { name: ".a", data: b"x" }]).is_err());
        // aarch64 is accepted
        assert!(build(&stub(0xAA64, false, true), "ID=x", "a", b"i", b"k").is_ok());
    }

    #[test]
    fn the_command_line_tool_builds_from_files_and_names_what_is_missing() {
        let d = crate::install::tests::tmp("uki-cli");
        let w = |n: &str, b: &[u8]| {
            std::fs::write(d.join(n), b).unwrap();
            d.join(n).display().to_string()
        };
        let (st, ki, ri, os) = (w("stub.efi", &stub(0x8664, false, true)), w("k", b"kernel"), w("i", b"initrd"), w("os", b"ID=iyakki\n"));
        let out = d.join("uki.efi").display().to_string();
        let a = |v: &[&str]| -> Vec<String> { v.iter().map(|s| s.to_string()).collect() };
        run_cli(&a(&["--uki", &st, "--linux", &ki, "--initrd", &ri, "--cmdline", "console=tty0", "--osrel", &os, "--out", &out])).unwrap();
        assert_eq!(sections(&std::fs::read(&out).unwrap()).unwrap().len(), 5);
        assert!(run_cli(&a(&["--uki", &st])).unwrap_err().contains("is needed"));
        assert!(run_cli(&a(&["--bogus"])).unwrap_err().contains("unknown option"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
