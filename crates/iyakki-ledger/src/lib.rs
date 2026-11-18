//! Execution replay ledger: an append-only, hash-chained log of inputs, tool
//! outputs, guest calls and linear-memory deltas. Supports integrity
//! verification and time-travel (reconstructing memory at any recorded call).

use sha2::{Digest, Sha256};

pub type Hash = [u8; 32];

pub fn hash_bytes(b: &[u8]) -> Hash {
    Sha256::digest(b).into()
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LedgerError {
    #[error("hash chain broken at entry {0}")]
    ChainBroken(usize),
    #[error("malformed ledger bytes")]
    Malformed,
    #[error("step {0} out of range")]
    NoSuchStep(usize),
}

/// Changed regions of linear memory between two points in time.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct MemDelta {
    pub new_len: u64,
    pub patches: Vec<(u64, Vec<u8>)>,
}

impl MemDelta {
    pub fn diff(old: &[u8], new: &[u8]) -> Self {
        const GAP: usize = 16; // merge runs separated by fewer unchanged bytes
        let mut patches: Vec<(u64, Vec<u8>)> = Vec::new();
        let mut i = 0;
        let mut run: Option<(usize, usize)> = None; // (start, end exclusive)
        let flush = |run: &mut Option<(usize, usize)>, patches: &mut Vec<(u64, Vec<u8>)>| {
            if let Some((s, e)) = run.take() {
                patches.push((s as u64, new[s..e].to_vec()));
            }
        };
        while i < new.len() {
            let changed = old.get(i) != Some(&new[i]);
            if changed {
                match &mut run {
                    Some((_, e)) if i - *e <= GAP => *e = i + 1,
                    _ => {
                        flush(&mut run, &mut patches);
                        run = Some((i, i + 1));
                    }
                }
            }
            i += 1;
        }
        flush(&mut run, &mut patches);
        MemDelta { new_len: new.len() as u64, patches }
    }

    pub fn apply(&self, mem: &mut Vec<u8>) {
        mem.resize(self.new_len as usize, 0);
        for (off, data) in &self.patches {
            let o = *off as usize;
            mem[o..o + data.len()].copy_from_slice(data);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// External input (e.g. prompt tokens) fed to the agent.
    Input(Vec<u8>),
    /// Output of a tool invocation.
    ToolOutput { tool: String, data: Vec<u8> },
    /// A host call made by the guest (name, path, status code).
    HostCall { name: String, path: String, status: i32 },
    /// A guest entrypoint call and its effect on memory.
    Call { entry: String, args: Vec<i32>, result: Option<i32>, fuel_used: u64, delta: MemDelta, mem_hash: Hash },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub event: Event,
    pub hash: Hash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ledger {
    /// Hash of the initial memory image the deltas apply to.
    pub base_hash: Hash,
    pub entries: Vec<Entry>,
}

// --- binary encoding -------------------------------------------------------

struct W(Vec<u8>);
impl W {
    fn u8(&mut self, v: u8) { self.0.push(v) }
    fn u32(&mut self, v: u32) { self.0.extend(v.to_le_bytes()) }
    fn u64(&mut self, v: u64) { self.0.extend(v.to_le_bytes()) }
    fn bytes(&mut self, b: &[u8]) { self.u64(b.len() as u64); self.0.extend(b) }
}
struct R<'a>(&'a [u8]);
impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], LedgerError> {
        if self.0.len() < n { return Err(LedgerError::Malformed); }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8, LedgerError> { Ok(self.take(1)?[0]) }
    fn u32(&mut self) -> Result<u32, LedgerError> { Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap())) }
    fn u64(&mut self) -> Result<u64, LedgerError> { Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap())) }
    fn bytes(&mut self) -> Result<Vec<u8>, LedgerError> {
        let n = self.u64()? as usize;
        Ok(self.take(n)?.to_vec())
    }
    fn string(&mut self) -> Result<String, LedgerError> { String::from_utf8(self.bytes()?).map_err(|_| LedgerError::Malformed) }
    fn hash(&mut self) -> Result<Hash, LedgerError> { Ok(self.take(32)?.try_into().unwrap()) }
}

fn encode_event(w: &mut W, e: &Event) {
    match e {
        Event::Input(b) => { w.u8(0); w.bytes(b) }
        Event::ToolOutput { tool, data } => { w.u8(1); w.bytes(tool.as_bytes()); w.bytes(data) }
        Event::HostCall { name, path, status } => {
            w.u8(2); w.bytes(name.as_bytes()); w.bytes(path.as_bytes()); w.u32(*status as u32)
        }
        Event::Call { entry, args, result, fuel_used, delta, mem_hash } => {
            w.u8(3);
            w.bytes(entry.as_bytes());
            w.u32(args.len() as u32);
            for a in args { w.u32(*a as u32) }
            match result { Some(r) => { w.u8(1); w.u32(*r as u32) } None => w.u8(0) }
            w.u64(*fuel_used);
            w.u64(delta.new_len);
            w.u32(delta.patches.len() as u32);
            for (o, d) in &delta.patches { w.u64(*o); w.bytes(d) }
            w.0.extend(mem_hash);
        }
    }
}

fn decode_event(r: &mut R) -> Result<Event, LedgerError> {
    Ok(match r.u8()? {
        0 => Event::Input(r.bytes()?),
        1 => Event::ToolOutput { tool: r.string()?, data: r.bytes()? },
        2 => Event::HostCall { name: r.string()?, path: r.string()?, status: r.u32()? as i32 },
        3 => {
            let entry = r.string()?;
            let n = r.u32()? as usize;
            let mut args = Vec::new();
            for _ in 0..n { args.push(r.u32()? as i32) }
            let result = if r.u8()? == 1 { Some(r.u32()? as i32) } else { None };
            let fuel_used = r.u64()?;
            let new_len = r.u64()?;
            let np = r.u32()? as usize;
            let mut patches = Vec::new();
            for _ in 0..np { patches.push((r.u64()?, r.bytes()?)) }
            Event::Call { entry, args, result, fuel_used, delta: MemDelta { new_len, patches }, mem_hash: r.hash()? }
        }
        _ => return Err(LedgerError::Malformed),
    })
}

fn chain_hash(prev: &Hash, e: &Event) -> Hash {
    let mut w = W(Vec::new());
    encode_event(&mut w, e);
    let mut h = Sha256::new();
    h.update(prev);
    h.update(&w.0);
    h.finalize().into()
}

impl Ledger {
    pub fn new(base_memory: &[u8]) -> Self {
        Self { base_hash: hash_bytes(base_memory), entries: Vec::new() }
    }

    fn tip(&self) -> Hash {
        self.entries.last().map_or(self.base_hash, |e| e.hash)
    }

    pub fn append(&mut self, event: Event) {
        let hash = chain_hash(&self.tip(), &event);
        self.entries.push(Entry { event, hash });
    }

    pub fn verify(&self) -> Result<(), LedgerError> {
        let mut prev = self.base_hash;
        for (i, e) in self.entries.iter().enumerate() {
            if chain_hash(&prev, &e.event) != e.hash {
                return Err(LedgerError::ChainBroken(i));
            }
            prev = e.hash;
        }
        Ok(())
    }

    /// The recorded `Call` events, in order.
    pub fn calls(&self) -> impl Iterator<Item = &Event> {
        self.entries.iter().map(|e| &e.event).filter(|e| matches!(e, Event::Call { .. }))
    }

    /// Memory image after the first `step` calls (0 = initial). Time travel.
    pub fn memory_at(&self, initial: &[u8], step: usize) -> Result<Vec<u8>, LedgerError> {
        if hash_bytes(initial) != self.base_hash {
            return Err(LedgerError::ChainBroken(0));
        }
        let total = self.calls().count();
        if step > total {
            return Err(LedgerError::NoSuchStep(step));
        }
        let mut mem = initial.to_vec();
        for ev in self.calls().take(step) {
            if let Event::Call { delta, mem_hash, .. } = ev {
                delta.apply(&mut mem);
                if hash_bytes(&mem) != *mem_hash {
                    return Err(LedgerError::ChainBroken(step));
                }
            }
        }
        Ok(mem)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = W(b"AFLG1".to_vec());
        w.0.extend(self.base_hash);
        w.u32(self.entries.len() as u32);
        for e in &self.entries {
            encode_event(&mut w, &e.event);
            w.0.extend(e.hash);
        }
        w.0
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self, LedgerError> {
        let mut r = R(b);
        if r.take(5)? != b"AFLG1" { return Err(LedgerError::Malformed); }
        let base_hash = r.hash()?;
        let n = r.u32()? as usize;
        let mut entries = Vec::new();
        for _ in 0..n {
            let event = decode_event(&mut r)?;
            entries.push(Entry { event, hash: r.hash()? });
        }
        let l = Ledger { base_hash, entries };
        l.verify()?;
        Ok(l)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_roundtrip() {
        let a = vec![0u8; 100];
        let mut b = a.clone();
        b[3] = 1;
        b[50..55].copy_from_slice(b"hello");
        b.extend([9, 9]);
        let d = MemDelta::diff(&a, &b);
        assert_eq!(d.patches.len(), 3);
        let mut c = a.clone();
        d.apply(&mut c);
        assert_eq!(c, b);
        // shrinking
        let d2 = MemDelta::diff(&b, &a);
        let mut e = b.clone();
        d2.apply(&mut e);
        assert_eq!(e, a);
    }

    fn sample() -> (Vec<u8>, Ledger) {
        let init = vec![0u8; 64];
        let mut l = Ledger::new(&init);
        l.append(Event::Input(b"prompt".to_vec()));
        let mut m1 = init.clone();
        m1[0] = 7;
        l.append(Event::Call { entry: "main".into(), args: vec![1], result: Some(3), fuel_used: 10,
            delta: MemDelta::diff(&init, &m1), mem_hash: hash_bytes(&m1) });
        l.append(Event::HostCall { name: "fs_write".into(), path: "/a".into(), status: 0 });
        let mut m2 = m1.clone();
        m2[1] = 8;
        l.append(Event::Call { entry: "main".into(), args: vec![], result: None, fuel_used: 5,
            delta: MemDelta::diff(&m1, &m2), mem_hash: hash_bytes(&m2) });
        (init, l)
    }

    #[test]
    fn verify_serialize_and_time_travel() {
        let (init, l) = sample();
        l.verify().unwrap();
        let l2 = Ledger::from_bytes(&l.to_bytes()).unwrap();
        assert_eq!(l, l2);
        assert_eq!(l.memory_at(&init, 0).unwrap(), init);
        assert_eq!(&l.memory_at(&init, 1).unwrap()[..2], &[7, 0]);
        assert_eq!(&l.memory_at(&init, 2).unwrap()[..2], &[7, 8]);
        assert_eq!(l.memory_at(&init, 3), Err(LedgerError::NoSuchStep(3)));
    }

    #[test]
    fn tampering_detected() {
        let (_, mut l) = sample();
        l.entries[0].event = Event::Input(b"evil".to_vec());
        assert_eq!(l.verify(), Err(LedgerError::ChainBroken(0)));
        let (_, l) = sample();
        let mut b = l.to_bytes();
        let n = b.len();
        b[n / 2] ^= 0xff;
        assert!(Ledger::from_bytes(&b).is_err());
    }
}
