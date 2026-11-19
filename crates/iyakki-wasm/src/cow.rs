//! F-WASM-14: the module a copy-on-write fork starts from. A snapshot's live memory is written into a copy of the guest's own module as
//! its one data segment (the memory section grown to hold it, the start function dropped because the snapshot is already past it), and
//! Wasmtime maps that segment copy-on-write when it instantiates the module (`memory_init_cow`, on by default on Linux). A fork of that
//! module then copies nothing at fork time: its pages are shared with the image until the child writes them.
//!
//! This is a byte-level rewriter of the binary format's sections, nothing else: no parser library. It keeps correct (F-WASM-15) a guest with
//! passive data segments (they stay where they are, so `memory.init` reads the same bytes at the same index; the active segments become empty
//! ones and the live memory is one more segment at the end) and a guest whose memory is imported (the import is dropped and the module
//! defines and exports the memory itself, sized to the live memory). A WAT text guest is turned into a binary module when its session is
//! made (`Sandbox::session`), so the rewriter always sees binary. It refuses, with a reason, what it still cannot keep correct: a shared,
//! 64-bit or custom-page memory, and more than one memory.

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

/// The module a copy-on-write image starts from, and where in its bytes the live memory sits (the one data segment's payload).
pub struct Rewritten {
    pub bytes: Vec<u8>,
    pub payload: std::ops::Range<usize>,
}

/// The number of segments in the data section of `wasm` (0 when there is none).
fn data_segment_count(wasm: &[u8]) -> Result<u32, String> {
    let mut at = 8usize;
    while at < wasm.len() {
        let id = wasm[at];
        at += 1;
        let len = leb_u32(wasm, &mut at)? as usize;
        let body = wasm.get(at..at + len).ok_or("a section runs past the end of the module")?;
        at += len;
        if id == 11 {
            let mut p = 0;
            return leb_u32(body, &mut p);
        }
    }
    Ok(0)
}

/// Read one constant offset expression (`i32.const`, `i64.const` or `global.get`, then `end`) and step past it.
fn skip_offset_expr(body: &[u8], p: &mut usize) -> Result<(), String> {
    let op = *body.get(*p).ok_or("damaged data section")?;
    *p += 1;
    match op {
        0x41 | 0x42 | 0x23 => {
            while *body.get(*p).ok_or("damaged data section")? & 0x80 != 0 {
                *p += 1;
            }
            *p += 1;
        }
        _ => return Err("a data segment offset this rewriter does not read".into()),
    }
    if body.get(*p) != Some(&0x0b) {
        return Err("a data segment offset is not a single constant".into());
    }
    *p += 1;
    Ok(())
}

/// `wasm` with `memory` as one more data segment at offset 0 (the others kept at their index, the active ones emptied) and the memory
/// section sized to `memory.len()` pages. `Err` names why not.
pub fn rewrite(wasm: &[u8], memory: &[u8]) -> Result<Vec<u8>, String> {
    rewrite_with_payload(wasm, memory).map(|r| r.bytes)
}

/// [`rewrite`], also saying where the live memory sits in the new module's bytes.
pub fn rewrite_with_payload(wasm: &[u8], memory: &[u8]) -> Result<Rewritten, String> {
    if wasm.len() < 8 || &wasm[..4] != b"\0asm" {
        return Err("the guest was loaded from text (WAT); a copy-on-write fork needs a binary module".into());
    }
    if memory.len() % PAGE != 0 || memory.is_empty() {
        return Err("the memory is not a whole number of pages".into());
    }
    let pages = (memory.len() / PAGE) as u32;
    let data_n = data_segment_count(wasm)?;
    let mut out = wasm[..8].to_vec();
    let (mut at, mut seen_memory, mut seen_data) = (8usize, false, false);
    // an imported memory: its limits flags and declared maximum; the module defines the memory itself in the image
    let mut imported: Option<(u8, Option<u32>)> = None;
    let mut payload = 0..0;
    let memory_section = |flags: u8, max: Option<u32>| {
        let mut b = vec![1u8, flags];
        put_leb(&mut b, pages);
        if let Some(m) = max {
            put_leb(&mut b, m);
        }
        b
    };
    while at < wasm.len() {
        let id = wasm[at];
        at += 1;
        let len = leb_u32(wasm, &mut at)? as usize;
        let body = wasm.get(at..at + len).ok_or("a section runs past the end of the module")?;
        at += len;
        // an imported memory has no memory section of its own: the defined one goes where the memory section belongs, before the globals
        if let Some((flags, max)) = imported {
            if !seen_memory && matches!(id, 6..=12) {
                put_section(&mut out, 5, &memory_section(flags, max));
                seen_memory = true;
            }
        }
        match id {
            2 => {
                // import section: the memory import (at most one) is dropped, every other import is kept as it is
                let mut p = 0;
                let n = leb_u32(body, &mut p)?;
                let mut kept: Vec<&[u8]> = Vec::new();
                for _ in 0..n {
                    let start = p;
                    for _ in 0..2 {
                        let l = leb_u32(body, &mut p)? as usize;
                        p += l;
                    }
                    let kind = *body.get(p).ok_or("damaged import")?;
                    p += 1;
                    let mut is_memory = false;
                    match kind {
                        2 => {
                            let flags = *body.get(p).ok_or("damaged import")?;
                            p += 1;
                            if flags & !1 != 0 {
                                return Err("a shared, 64-bit or custom-page memory".into());
                            }
                            let min = leb_u32(body, &mut p)?;
                            let max = if flags & 1 == 1 { Some(leb_u32(body, &mut p)?) } else { None };
                            if imported.is_some() {
                                return Err("the module does not have exactly one memory".into());
                            }
                            if max.is_some_and(|m| m < pages) || min > pages {
                                return Err("the memory's declared limits do not hold its current size".into());
                            }
                            imported = Some((flags, max));
                            is_memory = true;
                        }
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
                    if !is_memory {
                        kept.push(&body[start..p]);
                    }
                }
                let mut b = Vec::new();
                put_leb(&mut b, kept.len() as u32);
                for k in kept {
                    b.extend_from_slice(k);
                }
                put_section(&mut out, id, &b);
            }
            5 => {
                if imported.is_some() {
                    return Err("the module does not have exactly one memory".into());
                }
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
                put_section(&mut out, id, &memory_section(flags, max));
                seen_memory = true;
            }
            8 => {} // the start function already ran before the snapshot
            11 => {
                let mut p = 0;
                let n = leb_u32(body, &mut p)?;
                let mut b = Vec::new();
                put_leb(&mut b, n + 1);
                for _ in 0..n {
                    let start = p;
                    let kind = leb_u32(body, &mut p)?;
                    match kind {
                        1 => {
                            // passive: kept as it is, at the same index
                            let l = leb_u32(body, &mut p)? as usize;
                            p += l;
                            b.extend_from_slice(body.get(start..p).ok_or("damaged data section")?);
                        }
                        0 | 2 => {
                            if kind == 2 && leb_u32(body, &mut p)? != 0 {
                                return Err("a data segment of a memory other than the first".into());
                            }
                            skip_offset_expr(body, &mut p)?;
                            let l = leb_u32(body, &mut p)? as usize;
                            p += l;
                            // an active segment already did its work before the snapshot: it stays as an empty one so the indexes hold
                            b.extend_from_slice(&[0, 0x41, 0, 0x0b, 0]);
                        }
                        _ => return Err("a data segment of a kind this rewriter does not read".into()),
                    }
                }
                b.extend_from_slice(&[0, 0x41, 0, 0x0b]);
                put_leb(&mut b, memory.len() as u32);
                b.extend_from_slice(memory);
                // section header, then the body: the payload is the last `memory.len()` bytes of it
                out.push(id);
                put_leb(&mut out, b.len() as u32);
                out.extend_from_slice(&b);
                payload = (out.len() - memory.len())..out.len();
                seen_data = true;
            }
            12 => {
                let mut b = Vec::new();
                put_leb(&mut b, data_n + 1);
                put_section(&mut out, id, &b);
            }
            _ => put_section(&mut out, id, body),
        }
    }
    if !seen_memory {
        if let Some((flags, max)) = imported {
            put_section(&mut out, 5, &memory_section(flags, max));
        } else {
            return Err("the module has no memory section".into());
        }
    }
    if !seen_data {
        // no data section yet: it goes after the code section, which is the last non-custom section in the binary order
        let mut b = vec![1u8, 0, 0x41, 0, 0x0b];
        put_leb(&mut b, memory.len() as u32);
        b.extend_from_slice(memory);
        out.push(11);
        put_leb(&mut out, b.len() as u32);
        out.extend_from_slice(&b);
        payload = (out.len() - memory.len())..out.len();
    }
    Ok(Rewritten { bytes: out, payload })
}
