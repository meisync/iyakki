//! F-WASM-14: the module a copy-on-write fork starts from. A snapshot's live memory is written into a copy of the guest's own module as
//! its one data segment (the memory section grown to hold it, the start function dropped because the snapshot is already past it), and
//! Wasmtime maps that segment copy-on-write when it instantiates the module (`memory_init_cow`, on by default on Linux). A fork of that
//! module then copies nothing at fork time: its pages are shared with the image until the child writes them.
//!
//! This is a byte-level rewriter of the binary format's sections, nothing else: no parser library. It refuses (with a reason) what it cannot
//! keep correct: a module that is not a binary one (WAT text), an imported or shared or 64-bit memory, a passive data segment (a guest
//! that uses `memory.init` would read the wrong bytes), more than one memory.

fn leb_u32(b: &[u8], at: &mut usize) -> Result<u32, String> {
    let (mut r, mut shift) = (0u32, 0);
    loop {
        let x = *b.get(*at).ok_or("a number runs past the end of the module")?;
        *at += 1;
        if shift >= 35 {
            return Err("a number is too long".into());
        }
        r |= ((x & 0x7f) as u32) << shift;
        if x & 0x80 == 0 {
            return Ok(r);
        }
        shift += 7;
    }
}

fn put_leb(out: &mut Vec<u8>, mut v: u32) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn put_section(out: &mut Vec<u8>, id: u8, body: &[u8]) {
    out.push(id);
    put_leb(out, body.len() as u32);
    out.extend_from_slice(body);
}

/// The size of a WebAssembly page.
pub const PAGE: usize = 65536;

/// `wasm` with `memory` as its single data segment at offset 0 and the memory section sized to `memory.len()` pages. `Err` names why not.
pub fn rewrite(wasm: &[u8], memory: &[u8]) -> Result<Vec<u8>, String> {
    if wasm.len() < 8 || &wasm[..4] != b"\0asm" {
        return Err("the guest was loaded from text (WAT); a copy-on-write fork needs a binary module".into());
    }
    if memory.len() % PAGE != 0 || memory.is_empty() {
        return Err("the memory is not a whole number of pages".into());
    }
    let pages = (memory.len() / PAGE) as u32;
    let mut out = wasm[..8].to_vec();
    let (mut at, mut seen_memory, mut seen_data) = (8usize, false, false);
    while at < wasm.len() {
        let id = wasm[at];
        at += 1;
        let len = leb_u32(wasm, &mut at)? as usize;
        let body = wasm.get(at..at + len).ok_or("a section runs past the end of the module")?;
        at += len;
        match id {
            2 => {
                // import section: an imported memory is not ours to resize
                let mut p = 0;
                let n = leb_u32(body, &mut p)?;
                for _ in 0..n {
                    for _ in 0..2 {
                        let l = leb_u32(body, &mut p)? as usize;
                        p += l;
                    }
                    let kind = *body.get(p).ok_or("damaged import")?;
                    p += 1;
                    match kind {
                        2 => return Err("the memory is imported".into()),
                        0 => {
                            leb_u32(body, &mut p)?;
                        }
                        1 => {
                            p += 1; // reference type
                            let f = *body.get(p).ok_or("damaged import")?;
                            p += 1;
                            leb_u32(body, &mut p)?;
                            if f & 1 == 1 {
                                leb_u32(body, &mut p)?;
                            }
                        }
                        3 => p += 2, // value type, mutability
                        _ => return Err("an import of a kind this rewriter does not read".into()),
                    }
                }
                put_section(&mut out, id, body);
            }
            5 => {
                let mut p = 0;
                if leb_u32(body, &mut p)? != 1 {
                    return Err("the module does not have exactly one memory".into());
                }
                let flags = *body.get(p).ok_or("damaged memory section")?;
                p += 1;
                if flags & !1 != 0 {
                    return Err("a shared, 64-bit or custom-page memory".into());
                }
                let min = leb_u32(body, &mut p)?;
                let max = if flags & 1 == 1 { Some(leb_u32(body, &mut p)?) } else { None };
                if max.is_some_and(|m| m < pages) {
                    return Err("the memory's declared maximum is below its current size".into());
                }
                let _ = min;
                let mut b = vec![1u8, flags];
                put_leb(&mut b, pages);
                if let Some(m) = max {
                    put_leb(&mut b, m);
                }
                put_section(&mut out, id, &b);
                seen_memory = true;
            }
            8 => {} // the start function already ran before the snapshot
            11 => {
                let mut p = 0;
                let n = leb_u32(body, &mut p)?;
                for _ in 0..n {
                    let kind = leb_u32(body, &mut p)?;
                    if kind != 0 {
                        return Err("the guest has a passive or explicit-memory data segment".into());
                    }
                    // constant offset expression (i32.const, i64.const or global.get), then the bytes
                    let op = *body.get(p).ok_or("damaged data section")?;
                    p += 1;
                    match op {
                        0x41 | 0x42 | 0x23 => {
                            while *body.get(p).ok_or("damaged data section")? & 0x80 != 0 {
                                p += 1;
                            }
                            p += 1;
                        }
                        _ => return Err("a data segment offset this rewriter does not read".into()),
                    }
                    if body.get(p) != Some(&0x0b) {
                        return Err("a data segment offset is not a single constant".into());
                    }
                    p += 1;
                    let l = leb_u32(body, &mut p)? as usize;
                    p += l;
                }
                let mut b = vec![1u8, 0, 0x41, 0, 0x0b];
                put_leb(&mut b, memory.len() as u32);
                b.extend_from_slice(memory);
                put_section(&mut out, id, &b);
                seen_data = true;
            }
            12 => {
                let mut b = Vec::new();
                put_leb(&mut b, 1);
                put_section(&mut out, id, &b);
            }
            _ => put_section(&mut out, id, body),
        }
    }
    if !seen_memory {
        return Err("the module has no memory section".into());
    }
    if !seen_data {
        // no data section yet: it goes after the code section, which is the last non-custom section in the binary order
        let mut b = vec![1u8, 0, 0x41, 0, 0x0b];
        put_leb(&mut b, memory.len() as u32);
        b.extend_from_slice(memory);
        put_section(&mut out, 11, &b);
    }
    Ok(out)
}
