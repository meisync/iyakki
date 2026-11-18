//! Wasmtime sandbox: fuel-metered, memory-capped, capability-gated.
//!
//! Guests get no WASI. Their only window to the world is the `iyakki` import
//! module below, and every call that touches a resource must present a
//! [`SyscallToken`] previously granted to that guest.
//!
//! ABI (module `iyakki`, all pointers into guest memory `memory`):
//! - `log(ptr, len)`
//! - `token_get(index, out_ptr) -> i32`           copy granted token #index (16 bytes)
//! - `fs_read(tok_ptr, path_ptr, path_len, buf_ptr, buf_cap) -> i32`  bytes read, or <0
//! - `fs_write(tok_ptr, path_ptr, path_len, data_ptr, data_len) -> i32`  0, or <0
//! - `stdin_read(buf_ptr, buf_cap) -> i32`      next bytes of the input the host gave this run (0 at the end; needs `with_stdin`)
//! - `http_fetch(tok_ptr, url_ptr, url_len, body_ptr, body_len, out_ptr, out_cap) -> i32`  response bytes, or <0
//!   (GET if `body_len == 0`, else POST; needs a `NetConnect(host)` token and a configured [`NetBackend`])
//!
//! Error codes: see [`errno`].

use std::sync::{Arc, Mutex};

use iyakki_sec::{Access, CapabilityStore, SecError, SyscallToken};
use iyakki_ledger::{hash_bytes, Event, Ledger, MemDelta};
use iyakki_vfs::{MemFs, VfsError};
use wasmtime::{Caller, Config, Engine, Extern, Linker, Module, Store, StoreLimits, StoreLimitsBuilder, Trap};

pub mod cow;

pub mod errno {
    pub const BAD_MEMORY: i32 = -1;
    pub const DENIED: i32 = -2;
    pub const NOT_FOUND: i32 = -3;
    pub const IO: i32 = -4;
    pub const BUF_TOO_SMALL: i32 = -5;
    pub const BAD_INDEX: i32 = -6;
    /// Network request failed or no network backend is configured.
    pub const NET: i32 = -7;
}

/// Outbound network gateway. The supervisor supplies an implementation (see `iyakki-proxy`)
/// that applies policy, injects credentials and sanitizes responses outside the sandbox.
pub trait NetBackend: Send + Sync {
    /// `method` is "GET" or "POST". Returns the (sanitized) response body, or an errno.
    fn fetch(&self, method: &str, url: &str, body: &[u8]) -> Result<Vec<u8>, i32>;
}

#[derive(Debug, thiserror::Error)]
pub enum WasmError {
    #[error("fuel exhausted (CPU instruction limit hit)")]
    OutOfFuel,
    #[error("wall-clock limit hit (the guest ran longer than its time budget)")]
    Timeout,
    #[error("stopped: the person asked the run to stop")]
    Cancelled,
    #[error("guest trapped: {0}")]
    Trap(String),
    #[error("failed to load module: {0}")]
    Load(String),
    #[error("entrypoint `{0}` not found or has wrong signature")]
    NoEntry(String),
}

#[derive(Clone, Debug)]
pub struct SandboxConfig {
    /// Max wasm instructions (roughly) the guest may execute.
    pub fuel: u64,
    /// Max linear memory in bytes (growth beyond this fails inside the guest).
    pub max_memory_bytes: usize,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self { fuel: 100_000_000, max_memory_bytes: 64 * 1024 * 1024 }
    }
}

#[derive(Debug)]
pub struct RunOutcome {
    /// Return value of the entrypoint (`i32`) if it has one.
    pub result: Option<i32>,
    pub fuel_used: u64,
    /// Bytes the guest sent through `iyakki.log`.
    pub log: Vec<u8>,
}

struct HostState {
    vfs: Arc<Mutex<MemFs>>,
    caps: CapabilityStore,
    tokens: Vec<SyscallToken>,
    limits: StoreLimits,
    log: Vec<u8>,
    host_calls: Vec<Event>,
    net: Option<Arc<dyn NetBackend>>,
    stdin: Arc<Vec<u8>>,
    stdin_pos: usize,
}

/// Frozen state of a guest between calls: linear memory, exported mutable
/// globals, and the VFS. (Call stacks cannot be frozen mid-call; snapshots are
/// taken at call boundaries. Tables and non-exported globals are not captured.)
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub memory: Vec<u8>,
    pub globals: Vec<(String, GlobalVal)>,
    pub vfs: MemFs,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GlobalVal {
    I32(i32),
    I64(i64),
    F32(u32),
    F64(u64),
}

impl Snapshot {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut o = b"AFSN1".to_vec();
        o.extend((self.memory.len() as u64).to_le_bytes());
        o.extend(&self.memory);
        o.extend((self.globals.len() as u32).to_le_bytes());
        for (n, g) in &self.globals {
            o.extend((n.len() as u32).to_le_bytes());
            o.extend(n.as_bytes());
            let (t, v): (u8, u64) = match *g {
                GlobalVal::I32(x) => (0, x as u32 as u64),
                GlobalVal::I64(x) => (1, x as u64),
                GlobalVal::F32(x) => (2, x as u64),
                GlobalVal::F64(x) => (3, x),
            };
            o.push(t);
            o.extend(v.to_le_bytes());
        }
        o.extend(self.vfs.to_bytes());
        o
    }

    pub fn from_bytes(b: &[u8], vfs_quota: usize) -> Option<Snapshot> {
        let mut b = b.strip_prefix(b"AFSN1")?;
        fn take<'a>(b: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
            if b.len() < n { return None; }
            let (x, y) = b.split_at(n);
            *b = y;
            Some(x)
        }
        let ml = u64::from_le_bytes(take(&mut b, 8)?.try_into().ok()?) as usize;
        let memory = take(&mut b, ml)?.to_vec();
        let ng = u32::from_le_bytes(take(&mut b, 4)?.try_into().ok()?) as usize;
        let mut globals = Vec::new();
        for _ in 0..ng {
            let nl = u32::from_le_bytes(take(&mut b, 4)?.try_into().ok()?) as usize;
            let name = String::from_utf8(take(&mut b, nl)?.to_vec()).ok()?;
            let t = take(&mut b, 1)?[0];
            let v = u64::from_le_bytes(take(&mut b, 8)?.try_into().ok()?);
            let g = match t {
                0 => GlobalVal::I32(v as u32 as i32),
                1 => GlobalVal::I64(v as i64),
                2 => GlobalVal::F32(v as u32),
                3 => GlobalVal::F64(v),
                _ => return None,
            };
            globals.push((name, g));
        }
        let vfs = MemFs::from_bytes(b, vfs_quota).ok()?;
        Some(Snapshot { memory, globals, vfs })
    }
}

/// Size of one shared page of a [`PagedSnapshot`] (a WebAssembly page).
pub const PAGE: usize = 65536;

/// A snapshot whose memory is a list of shared pages (F-WASM-06, first real step). A page that
/// equals the same page of the parent snapshot (or is all zero) is the same `Arc`, so a tree of
/// snapshots stores only the pages that changed, and `restore_paged` writes only the pages that
/// differ from the live memory. This is NOT copy-on-write memory: taking a snapshot still reads
/// the whole live memory once (see docs/development/WASM-SNAPSHOTS.md for the measured numbers).
#[derive(Clone, Debug)]
pub struct PagedSnapshot {
    pages: Vec<Arc<Vec<u8>>>,
    len: usize,
    pub globals: Vec<(String, GlobalVal)>,
    pub vfs: MemFs,
}

impl PagedSnapshot {
    /// Bytes of guest memory the snapshot describes.
    pub fn memory_len(&self) -> usize {
        self.len
    }
    /// Number of pages.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }
    /// How many pages this snapshot shares (same allocation) with `other`.
    pub fn shared_pages_with(&self, other: &PagedSnapshot) -> usize {
        self.pages.iter().zip(&other.pages).filter(|(a, b)| Arc::ptr_eq(a, b)).count()
    }
    /// Bytes held only by this snapshot, not by any other holder of the same pages.
    pub fn unique_bytes(&self) -> usize {
        self.pages.iter().filter(|p| Arc::strong_count(p) == 1).map(|p| p.len()).sum()
    }
    /// The whole memory as one buffer (for tests and for converting to a plain [`Snapshot`]).
    pub fn memory(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(self.len);
        for p in &self.pages {
            v.extend_from_slice(p);
        }
        v
    }
    /// Convert to the plain, serialisable snapshot.
    pub fn to_snapshot(&self) -> Snapshot {
        Snapshot { memory: self.memory(), globals: self.globals.clone(), vfs: self.vfs.clone() }
    }
}

/// A host import added with [`Sandbox::with_import`]: (guest memory, i32 arguments) -> i32.
pub type HostImportFn = Arc<dyn Fn(&mut [u8], &[i32]) -> i32 + Send + Sync>;

#[derive(Clone)]
pub struct Sandbox {
    engine: Engine,
    cfg: SandboxConfig,
    linker: Arc<Linker<HostState>>,
    net: Option<Arc<dyn NetBackend>>,
    wall: Option<std::time::Duration>,
    stdin: Arc<Vec<u8>>,
    cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
}

impl Sandbox {
    pub fn new(cfg: SandboxConfig) -> Result<Self, WasmError> {
        let mut c = Config::new();
        c.consume_fuel(true);
        c.epoch_interruption(true);
        let engine = Engine::new(&c).map_err(|e| WasmError::Load(e.to_string()))?;
        let mut linker = Linker::new(&engine);
        register_host(&mut linker).map_err(|e| WasmError::Load(e.to_string()))?;
        Ok(Self { engine, cfg, linker: Arc::new(linker), net: None, wall: None, stdin: Arc::new(Vec::new()), cancel: None })
    }

    /// Route guest `iyakki.http_fetch` calls through `net`. Without one, they fail with `errno::NET`.
    pub fn with_net(mut self, net: Arc<dyn NetBackend>) -> Self {
        self.net = Some(net);
        self
    }

    /// Stop any call that runs longer than `d` of real time (in addition to the fuel limit; fuel does not
    /// count time spent in host functions such as `http_fetch`). The timer uses the engine's epoch, which
    /// every clone of this sandbox shares, so give each time-limited sandbox its own `Sandbox::new`.
    pub fn with_wall_time(mut self, d: std::time::Duration) -> Self {
        self.wall = Some(d);
        self
    }

    /// Input for the guest: it reads it with `iyakki.stdin_read` (at most 1 MiB is kept; nothing else is ever readable this way).
    pub fn with_stdin(mut self, input: Vec<u8>) -> Self {
        let mut input = input;
        input.truncate(1 << 20);
        self.stdin = Arc::new(input);
        self
    }

    /// Stop a call as soon as `flag` becomes true (checked every 20 ms, also while the guest is inside a host function that returns
    /// later). The call then ends with [`WasmError::Cancelled`]. Like the wall time it uses the engine's epoch: give each cancellable
    /// sandbox its own `Sandbox::new`.
    pub fn with_cancel(mut self, flag: Arc<std::sync::atomic::AtomicBool>) -> Self {
        self.cancel = Some(flag);
        self
    }

    /// Add one host import `module.name` taking `nparams` i32 parameters and returning one i32.
    /// `f` gets the guest's linear memory and the arguments. Used by hosts with their own import
    /// namespace (the tile host registers `iyakki_tile.*`). Additive: the built-in `iyakki.*` imports stay.
    pub fn with_import(mut self, module: &str, name: &str, nparams: usize, f: HostImportFn) -> Self {
        let ty = wasmtime::FuncType::new(
            &self.engine,
            vec![wasmtime::ValType::I32; nparams],
            [wasmtime::ValType::I32],
        );
        let mut linker = (*self.linker).clone();
        let r = linker.func_new(module, name, ty, move |mut c, params, results| {
            let args: Vec<i32> = params.iter().map(|p| p.i32().unwrap_or(0)).collect();
            let mem = match c.get_export("memory") {
                Some(Extern::Memory(m)) => m,
                _ => {
                    results[0] = wasmtime::Val::I32(errno::BAD_MEMORY);
                    return Ok(());
                }
            };
            let v = f(mem.data_mut(&mut c), &args);
            results[0] = wasmtime::Val::I32(v);
            Ok(())
        });
        if r.is_ok() {
            self.linker = Arc::new(linker);
        }
        self
    }

    /// Instantiate a long-lived guest session (does not call any export).
    pub fn session(
        &self,
        wasm: &[u8],
        vfs: Arc<Mutex<MemFs>>,
        caps: CapabilityStore,
        tokens: Vec<SyscallToken>,
    ) -> Result<Session, WasmError> {
        let module = Module::new(&self.engine, wasm).map_err(|e| WasmError::Load(e.to_string()))?;
        self.instantiate(module, Arc::new(wasm.to_vec()), vfs, caps, tokens)
    }

    fn instantiate(
        &self,
        module: Module,
        wasm: Arc<Vec<u8>>,
        vfs: Arc<Mutex<MemFs>>,
        caps: CapabilityStore,
        tokens: Vec<SyscallToken>,
    ) -> Result<Session, WasmError> {
        let limits = StoreLimitsBuilder::new()
            .memory_size(self.cfg.max_memory_bytes)
            .instances(1)
            .memories(1)
            .tables(4)
            .build();
        let mut store = Store::new(
            &self.engine,
            HostState { vfs, caps, tokens, limits, log: Vec::new(), host_calls: Vec::new(), net: self.net.clone(), stdin: self.stdin.clone(), stdin_pos: 0 },
        );
        store.limiter(|s| &mut s.limits);
        store.set_fuel(self.cfg.fuel).map_err(|e| WasmError::Load(e.to_string()))?;
        store.set_epoch_deadline(u64::MAX / 4);
        let instance = self.linker.instantiate(&mut store, &module).map_err(classify)?;
        Ok(Session { sandbox: self.clone(), module, wasm, store, instance, ledger: None })
    }

    /// One-shot: run `entry` (no params, optional `i32` result) from wasm binary or WAT text.
    pub fn run(
        &self,
        wasm: &[u8],
        entry: &str,
        vfs: Arc<Mutex<MemFs>>,
        caps: CapabilityStore,
        tokens: Vec<SyscallToken>,
    ) -> Result<RunOutcome, WasmError> {
        self.session(wasm, vfs, caps, tokens)?.call(entry, &[])
    }
}

/// The state of a session as a module whose memory image is copy-on-write (see [`Session::cow_image`]).
pub struct CowImage {
    sandbox: Sandbox,
    module: Module,
    wasm: Arc<Vec<u8>>,
    globals: Vec<(String, GlobalVal)>,
    vfs: MemFs,
    caps: CapabilityStore,
    tokens: Vec<SyscallToken>,
}

impl CowImage {
    /// A new session at the image's state. The memory is not copied here: Wasmtime maps the image copy-on-write.
    pub fn fork(&self) -> Result<Session, WasmError> {
        let vfs = Arc::new(Mutex::new(self.vfs.fork()));
        let mut s = self.sandbox.instantiate(self.module.clone(), self.wasm.clone(), vfs, self.caps.clone(), self.tokens.clone())?;
        s.restore_globals_and_vfs(&self.globals, &self.vfs)?;
        Ok(s)
    }
}

pub struct Session {
    sandbox: Sandbox,
    module: Module,
    /// The bytes the module was made from (a copy-on-write fork rewrites them, F-WASM-14).
    wasm: Arc<Vec<u8>>,
    store: Store<HostState>,
    instance: wasmtime::Instance,
    ledger: Option<Ledger>,
}

impl Session {
    fn memory(&mut self) -> Option<wasmtime::Memory> {
        self.instance.get_memory(&mut self.store, "memory")
    }

    pub fn memory_bytes(&mut self) -> Vec<u8> {
        self.memory().map(|m| m.data(&self.store).to_vec()).unwrap_or_default()
    }

    /// Copy `data` into guest linear memory at `ptr`. Errors if out of bounds.
    pub fn write_memory(&mut self, ptr: u32, data: &[u8]) -> Result<(), WasmError> {
        let mem = self.memory().ok_or_else(|| WasmError::Trap("no exported memory".into()))?;
        mem.write(&mut self.store, ptr as usize, data).map_err(|e| WasmError::Trap(e.to_string()))
    }

    /// Read `len` bytes of guest linear memory at `ptr`.
    pub fn read_memory(&mut self, ptr: u32, len: usize) -> Result<Vec<u8>, WasmError> {
        let mem = self.memory().ok_or_else(|| WasmError::Trap("no exported memory".into()))?;
        let mut v = vec![0u8; len];
        mem.read(&self.store, ptr as usize, &mut v).map_err(|e| WasmError::Trap(e.to_string()))?;
        Ok(v)
    }

    pub fn vfs(&self) -> Arc<Mutex<MemFs>> {
        self.store.data().vfs.clone()
    }

    /// Start recording calls into a replay ledger (based on the current memory).
    pub fn start_recording(&mut self) {
        let m = self.memory_bytes();
        self.ledger = Some(Ledger::new(&m));
    }

    pub fn ledger(&self) -> Option<&Ledger> {
        self.ledger.as_ref()
    }

    pub fn take_ledger(&mut self) -> Option<Ledger> {
        self.ledger.take()
    }

    /// Record an external input (e.g. prompt) in the ledger, if recording.
    pub fn record_input(&mut self, data: &[u8]) {
        if let Some(l) = &mut self.ledger {
            l.append(Event::Input(data.to_vec()));
        }
    }

    pub fn record_tool_output(&mut self, tool: &str, data: &[u8]) {
        if let Some(l) = &mut self.ledger {
            l.append(Event::ToolOutput { tool: tool.into(), data: data.to_vec() });
        }
    }

    /// Call an exported function taking `args.len()` i32 params, returning at most one i32.
    /// Fuel is refilled to the configured budget for each call.
    pub fn call(&mut self, entry: &str, args: &[i32]) -> Result<RunOutcome, WasmError> {
        let budget = self.sandbox.cfg.fuel;
        self.store.set_fuel(budget).map_err(|e| WasmError::Load(e.to_string()))?;
        let func = self
            .instance
            .get_func(&mut self.store, entry)
            .ok_or_else(|| WasmError::NoEntry(entry.into()))?;
        let ty = func.ty(&self.store);
        let ok_params = ty.params().len() == args.len() && ty.params().all(|p| matches!(p, wasmtime::ValType::I32));
        if !ok_params || ty.results().len() > 1 {
            return Err(WasmError::NoEntry(entry.into()));
        }
        let has_result = ty.results().len() == 1;
        let before = if self.ledger.is_some() { self.memory_bytes() } else { Vec::new() };
        let params: Vec<wasmtime::Val> = args.iter().map(|a| wasmtime::Val::I32(*a)).collect();
        let mut results = [wasmtime::Val::I32(0)];
        let n = usize::from(has_result);
        // the watchdog bumps the engine's epoch when the time is up; dropping `_done` ends it early
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        if self.sandbox.wall.is_some() || self.sandbox.cancel.is_some() {
            self.store.set_epoch_deadline(1);
            let engine = self.sandbox.engine.clone();
            let (wall, cancel) = (self.sandbox.wall, self.sandbox.cancel.clone());
            std::thread::spawn(move || {
                let start = std::time::Instant::now();
                loop {
                    if done_rx.recv_timeout(std::time::Duration::from_millis(20)) != Err(std::sync::mpsc::RecvTimeoutError::Timeout) {
                        return;
                    }
                    if cancel.as_ref().is_some_and(|c| c.load(std::sync::atomic::Ordering::SeqCst)) || wall.is_some_and(|d| start.elapsed() >= d) {
                        engine.increment_epoch();
                        return;
                    }
                }
            });
        }
        let call_res = func.call(&mut self.store, &params, &mut results[..n]);
        drop(done_tx);
        self.store.set_epoch_deadline(u64::MAX / 4);
        let remaining = self.store.get_fuel().unwrap_or(0);
        let host_calls = std::mem::take(&mut self.store.data_mut().host_calls);
        let log = std::mem::take(&mut self.store.data_mut().log);
        let cancelled = self.sandbox.cancel.as_ref().is_some_and(|c| c.load(std::sync::atomic::Ordering::SeqCst));
        call_res.map_err(|e| match classify(e) {
            WasmError::Timeout if cancelled => WasmError::Cancelled,
            other => other,
        })?;
        let result = match (has_result, &results[0]) {
            (true, wasmtime::Val::I32(v)) => Some(*v),
            (true, _) => return Err(WasmError::NoEntry(entry.into())),
            _ => None,
        };
        let fuel_used = budget - remaining;
        if self.ledger.is_some() {
            let after = self.memory_bytes();
            let l = self.ledger.as_mut().unwrap();
            for h in host_calls {
                l.append(h);
            }
            l.append(Event::Call {
                entry: entry.into(),
                args: args.to_vec(),
                result,
                fuel_used,
                delta: MemDelta::diff(&before, &after),
                mem_hash: hash_bytes(&after),
            });
        }
        Ok(RunOutcome { result, fuel_used, log })
    }

    pub fn snapshot(&mut self) -> Snapshot {
        let memory = self.memory_bytes();
        let mut globals = Vec::new();
        let names: Vec<String> = self.instance.exports(&mut self.store).map(|e| e.name().to_string()).collect();
        for n in names {
            if let Some(g) = self.instance.get_global(&mut self.store, &n) {
                if g.ty(&self.store).mutability() == wasmtime::Mutability::Var {
                    let v = match g.get(&mut self.store) {
                        wasmtime::Val::I32(x) => GlobalVal::I32(x),
                        wasmtime::Val::I64(x) => GlobalVal::I64(x),
                        wasmtime::Val::F32(x) => GlobalVal::F32(x),
                        wasmtime::Val::F64(x) => GlobalVal::F64(x),
                        _ => continue,
                    };
                    globals.push((n, v));
                }
            }
        }
        let vfs = self.store.data().vfs.lock().unwrap().fork();
        Snapshot { memory, globals, vfs }
    }

    /// Restore memory, globals and VFS contents from a snapshot taken of the same module.
    pub fn restore(&mut self, snap: &Snapshot) -> Result<(), WasmError> {
        if let Some(mem) = self.memory() {
            let cur = mem.data_size(&self.store);
            if snap.memory.len() > cur {
                let pages = (snap.memory.len() - cur).div_ceil(65536) as u64;
                mem.grow(&mut self.store, pages).map_err(|e| WasmError::Trap(e.to_string()))?;
            }
            let data = mem.data_mut(&mut self.store);
            if snap.memory.len() > data.len() {
                return Err(WasmError::Trap("snapshot larger than memory".into()));
            }
            data[..snap.memory.len()].copy_from_slice(&snap.memory);
            // zero anything beyond the snapshot (memory cannot shrink)
            data[snap.memory.len()..].fill(0);
        }
        for (n, v) in &snap.globals {
            if let Some(g) = self.instance.get_global(&mut self.store, n) {
                let val = match *v {
                    GlobalVal::I32(x) => wasmtime::Val::I32(x),
                    GlobalVal::I64(x) => wasmtime::Val::I64(x),
                    GlobalVal::F32(x) => wasmtime::Val::F32(x),
                    GlobalVal::F64(x) => wasmtime::Val::F64(x),
                };
                g.set(&mut self.store, val).map_err(|e| WasmError::Trap(e.to_string()))?;
            }
        }
        self.store.data().vfs.lock().unwrap().replace_with(snap.vfs.fork());
        Ok(())
    }

    /// New independent session at the same state: own memory, own forked VFS.
    pub fn fork(&mut self) -> Result<Session, WasmError> {
        let mut globals = Vec::new();
        self.collect_globals(&mut globals);
        let vfs_copy = self.store.data().vfs.lock().unwrap_or_else(std::sync::PoisonError::into_inner).fork();
        let d = self.store.data();
        let (caps, tokens) = (d.caps.clone(), d.tokens.clone());
        let vfs = Arc::new(Mutex::new(vfs_copy.fork()));
        let mut s = self.sandbox.instantiate(self.module.clone(), self.wasm.clone(), vfs, caps, tokens)?;
        // one copy straight from the parent's memory into the child's (no intermediate buffer)
        if let (Some(src), Some(dst)) = (self.memory(), s.memory()) {
            let want = src.data_size(&self.store);
            let cur = dst.data_size(&s.store);
            if want > cur {
                dst.grow(&mut s.store, (want - cur).div_ceil(PAGE) as u64).map_err(|e| WasmError::Trap(e.to_string()))?;
            }
            let from = src.data(&self.store);
            let to = dst.data_mut(&mut s.store);
            to[..from.len()].copy_from_slice(from);
            to[from.len()..].fill(0);
        }
        s.restore_globals_and_vfs(&globals, &vfs_copy)?;
        Ok(s)
    }

    /// `n` independent sessions at this state. A big guest forked many times goes through a copy-on-write image (see [`image_pays`]);
    /// a small one, a few forks, or a guest that cannot have an image (WAT text, imported memory, passive data) is copied.
    pub fn fork_many(&mut self, n: usize) -> Result<Vec<Session>, WasmError> {
        let size = self.memory().map_or(0, |m| m.data_size(&self.store));
        if image_pays(n, size) {
            if let Ok(image) = self.cow_image() {
                return (0..n).map(|_| image.fork()).collect();
            }
        }
        (0..n).map(|_| self.fork()).collect()
    }

    /// A copy-on-write image of this session's state (F-WASM-14): the live memory becomes the data segment of a copy of the module and
    /// Wasmtime maps it copy-on-write, so every fork of the image copies no memory at fork time. Making the image costs one module build
    /// (compile and one copy of the memory into the image); it pays when many forks start from one state (the agent's what-if runs).
    /// `Err(Load)` says why a guest cannot have one (text module, imported or shared memory, passive data segments).
    pub fn cow_image(&mut self) -> Result<CowImage, WasmError> {
        let mem = self.memory_bytes();
        let rewritten = cow::rewrite(&self.wasm, &mem).map_err(WasmError::Load)?;
        let module = Module::new(&self.sandbox.engine, &rewritten).map_err(|e| WasmError::Load(e.to_string()))?;
        let mut globals = Vec::new();
        self.collect_globals(&mut globals);
        let vfs = self.store.data().vfs.lock().unwrap_or_else(std::sync::PoisonError::into_inner).fork();
        let d = self.store.data();
        Ok(CowImage { sandbox: self.sandbox.clone(), module, wasm: Arc::new(rewritten), globals, vfs, caps: d.caps.clone(), tokens: d.tokens.clone() })
    }

    /// Snapshot into shared pages. With a `parent`, pages equal to the parent's page are shared with it.
    pub fn snapshot_paged(&mut self, parent: Option<&PagedSnapshot>) -> PagedSnapshot {
        let mut globals = Vec::new();
        self.collect_globals(&mut globals);
        let vfs = self.store.data().vfs.lock().unwrap_or_else(std::sync::PoisonError::into_inner).fork();
        let zero: Arc<Vec<u8>> = Arc::new(vec![0u8; PAGE]);
        let mut pages: Vec<Arc<Vec<u8>>> = Vec::new();
        let mut len = 0usize;
        if let Some(mem) = self.memory() {
            let data = mem.data(&self.store);
            len = data.len();
            for (i, chunk) in data.chunks(PAGE).enumerate() {
                let shared = parent
                    .and_then(|p| p.pages.get(i))
                    .filter(|pp| pp.as_slice() == chunk)
                    .cloned()
                    .or_else(|| (chunk.len() == PAGE && chunk.iter().all(|b| *b == 0)).then(|| zero.clone()));
                pages.push(shared.unwrap_or_else(|| Arc::new(chunk.to_vec())));
            }
        }
        PagedSnapshot { pages, len, globals, vfs }
    }

    /// Restore from a paged snapshot, writing only the pages whose bytes differ from the live memory.
    /// Returns the number of pages written.
    pub fn restore_paged(&mut self, snap: &PagedSnapshot) -> Result<usize, WasmError> {
        let mut written = 0usize;
        if let Some(mem) = self.memory() {
            let cur = mem.data_size(&self.store);
            if snap.len > cur {
                let pages = (snap.len - cur).div_ceil(PAGE) as u64;
                mem.grow(&mut self.store, pages).map_err(|e| WasmError::Trap(e.to_string()))?;
            }
            let data = mem.data_mut(&mut self.store);
            if snap.len > data.len() {
                return Err(WasmError::Trap("snapshot larger than memory".into()));
            }
            for (i, page) in snap.pages.iter().enumerate() {
                let at = i * PAGE;
                let dst = &mut data[at..at + page.len()];
                if dst != page.as_slice() {
                    dst.copy_from_slice(page);
                    written += 1;
                }
            }
            data[snap.len..].fill(0);
        }
        self.restore_globals_and_vfs(&snap.globals, &snap.vfs)?;
        Ok(written)
    }

    /// Fork from a paged snapshot (a new instance, then only the pages that differ from its fresh memory are written).
    pub fn fork_from_paged(&mut self, snap: &PagedSnapshot) -> Result<Session, WasmError> {
        let d = self.store.data();
        let (caps, tokens) = (d.caps.clone(), d.tokens.clone());
        let vfs = Arc::new(Mutex::new(snap.vfs.fork()));
        let mut s = self.sandbox.instantiate(self.module.clone(), self.wasm.clone(), vfs, caps, tokens)?;
        s.restore_paged(snap)?;
        Ok(s)
    }

    fn collect_globals(&mut self, out: &mut Vec<(String, GlobalVal)>) {
        let names: Vec<String> = self.instance.exports(&mut self.store).map(|e| e.name().to_string()).collect();
        for n in names {
            if let Some(g) = self.instance.get_global(&mut self.store, &n) {
                if g.ty(&self.store).mutability() == wasmtime::Mutability::Var {
                    let v = match g.get(&mut self.store) {
                        wasmtime::Val::I32(x) => GlobalVal::I32(x),
                        wasmtime::Val::I64(x) => GlobalVal::I64(x),
                        wasmtime::Val::F32(x) => GlobalVal::F32(x),
                        wasmtime::Val::F64(x) => GlobalVal::F64(x),
                        _ => continue,
                    };
                    out.push((n, v));
                }
            }
        }
    }

    fn restore_globals_and_vfs(&mut self, globals: &[(String, GlobalVal)], vfs: &MemFs) -> Result<(), WasmError> {
        for (n, v) in globals {
            if let Some(g) = self.instance.get_global(&mut self.store, n) {
                let val = match *v {
                    GlobalVal::I32(x) => wasmtime::Val::I32(x),
                    GlobalVal::I64(x) => wasmtime::Val::I64(x),
                    GlobalVal::F32(x) => wasmtime::Val::F32(x),
                    GlobalVal::F64(x) => wasmtime::Val::F64(x),
                };
                g.set(&mut self.store, val).map_err(|e| WasmError::Trap(e.to_string()))?;
            }
        }
        self.store.data().vfs.lock().unwrap_or_else(std::sync::PoisonError::into_inner).replace_with(vfs.fork());
        Ok(())
    }

    /// Replace this session's state with `winner`'s (commit a speculative branch).
    pub fn commit(&mut self, winner: &mut Session) -> Result<(), WasmError> {
        let snap = winner.snapshot();
        self.restore(&snap)
    }
}

/// From this many forks of one state on, and from this much memory, making a copy-on-write image first costs less than copying the memory
/// for every fork (measured, `cow_fork_timing`: making the image costs about as much as 20 to 25 copies at 16 to 64 MiB).
pub const COW_FORKS: usize = 24;
pub const COW_MIN_MEMORY: usize = 16 << 20;

/// Whether `n` forks of a guest with `memory` bytes are cheaper from an image.
pub fn image_pays(n: usize, memory: usize) -> bool {
    n >= COW_FORKS && memory >= COW_MIN_MEMORY
}

/// Outcome of one speculative branch.
#[derive(Debug)]
pub struct BranchResult {
    pub index: usize,
    pub outcome: Result<RunOutcome, String>,
}

/// Fork `parent` into `n` branches, run `entry(branch_index)` on each in parallel,
/// score them with `scorer` (higher wins; failed branches never win; ties go to the
/// lowest index, so the choice is deterministic) and commit the winner into `parent`.
/// Returns the winning index and all branch results, or `None` if every branch failed.
pub fn speculate<F>(
    parent: &mut Session,
    n: usize,
    entry: &str,
    scorer: F,
) -> Result<Option<(usize, Vec<BranchResult>)>, WasmError>
where
    F: Fn(&BranchResult, &mut Session) -> f64 + Sync,
{
    let mut branches = parent.fork_many(n)?;
    let results: Vec<BranchResult> = std::thread::scope(|sc| {
        let hs: Vec<_> = branches
            .iter_mut()
            .enumerate()
            .map(|(i, b)| {
                sc.spawn(move || BranchResult { index: i, outcome: b.call(entry, &[i as i32]).map_err(|e| e.to_string()) })
            })
            .collect();
        hs.into_iter().map(|h| h.join().expect("branch thread panicked")).collect()
    });
    let mut best: Option<(usize, f64)> = None;
    for (i, r) in results.iter().enumerate() {
        if r.outcome.is_err() {
            continue;
        }
        let sc = scorer(r, &mut branches[i]);
        if sc.is_nan() {
            continue;
        }
        if best.is_none_or(|(_, b)| sc > b) {
            best = Some((i, sc));
        }
    }
    match best {
        Some((i, _)) => {
            parent.commit(&mut branches[i])?;
            Ok(Some((i, results)))
        }
        None => Ok(None),
    }
}

fn classify(e: wasmtime::Error) -> WasmError {
    match e.downcast_ref::<Trap>() {
        Some(Trap::OutOfFuel) => WasmError::OutOfFuel,
        Some(Trap::Interrupt) => WasmError::Timeout,
        _ => WasmError::Trap(format!("{e:#}")),
    }
}

fn mem_slice<'a>(c: &'a mut Caller<'_, HostState>, ptr: i32, len: i32) -> Option<(&'a mut [u8], &'a mut HostState)> {
    let mem = match c.get_export("memory") {
        Some(Extern::Memory(m)) => m,
        _ => return None,
    };
    let (ptr, len) = (u32::try_from(ptr).ok()? as usize, u32::try_from(len).ok()? as usize);
    let (data, state) = mem.data_and_store_mut(c);
    data.get_mut(ptr..ptr.checked_add(len)?).map(|s| (s, state))
}

fn read_token(c: &mut Caller<'_, HostState>, ptr: i32) -> Option<SyscallToken> {
    let (b, _) = mem_slice(c, ptr, SyscallToken::LEN as i32)?;
    Some(SyscallToken::from_bytes(b.try_into().ok()?))
}

fn read_str(c: &mut Caller<'_, HostState>, ptr: i32, len: i32) -> Option<String> {
    let (b, _) = mem_slice(c, ptr, len)?;
    String::from_utf8(b.to_vec()).ok()
}

fn sec_err(e: SecError) -> i32 {
    let _ = e;
    errno::DENIED
}

fn vfs_err(e: VfsError) -> i32 {
    match e {
        VfsError::NotFound(_) => errno::NOT_FOUND,
        VfsError::InvalidPath(_) => errno::DENIED,
        _ => errno::IO,
    }
}

fn register_host(l: &mut Linker<HostState>) -> wasmtime::Result<()> {
    l.func_wrap("iyakki", "log", |mut c: Caller<'_, HostState>, ptr: i32, len: i32| {
        if let Some((b, st)) = mem_slice(&mut c, ptr, len) {
            let b = b.to_vec();
            // Cap log growth so a guest cannot exhaust host memory via logging.
            if st.log.len() + b.len() <= 1 << 20 {
                st.log.extend_from_slice(&b);
            }
        }
    })?;

    l.func_wrap("iyakki", "stdin_read", |mut c: Caller<'_, HostState>, ptr: i32, cap: i32| -> i32 {
        let Some((b, st)) = mem_slice(&mut c, ptr, cap) else { return errno::BAD_MEMORY };
        let rest = st.stdin.get(st.stdin_pos..).unwrap_or(&[]);
        let n = rest.len().min(b.len());
        b[..n].copy_from_slice(&rest[..n]);
        st.stdin_pos += n;
        n as i32
    })?;

    l.func_wrap("iyakki", "token_get", |mut c: Caller<'_, HostState>, idx: i32, out: i32| -> i32 {
        let tok = match usize::try_from(idx).ok().and_then(|i| c.data().tokens.get(i).copied()) {
            Some(t) => t,
            None => return errno::BAD_INDEX,
        };
        match mem_slice(&mut c, out, SyscallToken::LEN as i32) {
            Some((b, _)) => {
                b.copy_from_slice(tok.as_bytes());
                0
            }
            None => errno::BAD_MEMORY,
        }
    })?;

    l.func_wrap(
        "iyakki",
        "fs_read",
        |mut c: Caller<'_, HostState>, tp: i32, pp: i32, pl: i32, bp: i32, bc: i32| -> i32 {
            let (Some(tok), Some(path)) = (read_token(&mut c, tp), read_str(&mut c, pp, pl)) else {
                return errno::BAD_MEMORY;
            };
            let norm = match iyakki_vfs::normalize(&path) {
                Ok(p) => p,
                Err(e) => return vfs_err(e),
            };
            let rc = (|| {
                if let Err(e) = c.data().caps.check(&tok, &Access::FsRead(&norm)) {
                    return sec_err(e);
                }
                let data = match c.data().vfs.lock().unwrap().read(&norm) {
                    Ok(d) => d,
                    Err(e) => return vfs_err(e),
                };
                if data.len() > bc.max(0) as usize {
                    return errno::BUF_TOO_SMALL;
                }
                match mem_slice(&mut c, bp, bc) {
                    Some((b, _)) => {
                        b[..data.len()].copy_from_slice(&data);
                        data.len() as i32
                    }
                    None => errno::BAD_MEMORY,
                }
            })();
            c.data_mut().host_calls.push(Event::HostCall { name: "fs_read".into(), path: norm, status: rc });
            rc
        },
    )?;

    l.func_wrap(
        "iyakki",
        "fs_write",
        |mut c: Caller<'_, HostState>, tp: i32, pp: i32, pl: i32, dp: i32, dl: i32| -> i32 {
            let (Some(tok), Some(path)) = (read_token(&mut c, tp), read_str(&mut c, pp, pl)) else {
                return errno::BAD_MEMORY;
            };
            let norm = match iyakki_vfs::normalize(&path) {
                Ok(p) => p,
                Err(e) => return vfs_err(e),
            };
            let rc = (|| {
                if let Err(e) = c.data().caps.check(&tok, &Access::FsWrite(&norm)) {
                    return sec_err(e);
                }
                let data = match mem_slice(&mut c, dp, dl) {
                    Some((b, _)) => b.to_vec(),
                    None => return errno::BAD_MEMORY,
                };
                match c.data().vfs.lock().unwrap().write(&norm, &data) {
                    Ok(()) => 0,
                    Err(e) => vfs_err(e),
                }
            })();
            c.data_mut().host_calls.push(Event::HostCall { name: "fs_write".into(), path: norm, status: rc });
            rc
        },
    )?;

    l.func_wrap(
        "iyakki",
        "http_fetch",
        |mut c: Caller<'_, HostState>, tp: i32, up: i32, ul: i32, bp: i32, bl: i32, op: i32, oc: i32| -> i32 {
            let (Some(tok), Some(url)) = (read_token(&mut c, tp), read_str(&mut c, up, ul)) else {
                return errno::BAD_MEMORY;
            };
            let rc = (|| {
                let Some(host) = iyakki_sec::url_host(&url) else { return errno::DENIED };
                if let Err(e) = c.data().caps.check(&tok, &Access::NetConnect(&host)) {
                    return sec_err(e);
                }
                let body = if bl > 0 {
                    match mem_slice(&mut c, bp, bl) {
                        Some((b, _)) => b.to_vec(),
                        None => return errno::BAD_MEMORY,
                    }
                } else {
                    Vec::new()
                };
                let Some(net) = c.data().net.clone() else { return errno::NET };
                let method = if bl > 0 { "POST" } else { "GET" };
                let resp = match net.fetch(method, &url, &body) {
                    Ok(r) => r,
                    Err(e) => return e,
                };
                if resp.len() > oc.max(0) as usize {
                    return errno::BUF_TOO_SMALL;
                }
                match mem_slice(&mut c, op, oc) {
                    Some((b, _)) => {
                        b[..resp.len()].copy_from_slice(&resp);
                        resp.len() as i32
                    }
                    None => errno::BAD_MEMORY,
                }
            })();
            c.data_mut().host_calls.push(Event::HostCall { name: "http_fetch".into(), path: url, status: rc });
            rc
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_sec::Capability;

    fn setup() -> (Arc<Mutex<MemFs>>, CapabilityStore) {
        (Arc::new(Mutex::new(MemFs::new())), CapabilityStore::new())
    }

    #[test]
    fn runs_simple_module() {
        let (fs, caps) = setup();
        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let wat = r#"(module (func (export "main") (result i32) i32.const 42))"#;
        let out = sb.run(wat.as_bytes(), "main", fs, caps, vec![]).unwrap();
        assert_eq!(out.result, Some(42));
        assert!(out.fuel_used > 0);
    }

    #[test]
    fn infinite_loop_runs_out_of_fuel() {
        let (fs, caps) = setup();
        let sb = Sandbox::new(SandboxConfig { fuel: 10_000, ..Default::default() }).unwrap();
        let wat = r#"(module (func (export "main") (loop br 0)))"#;
        assert!(matches!(sb.run(wat.as_bytes(), "main", fs, caps, vec![]), Err(WasmError::OutOfFuel)));
    }

    #[test]
    fn the_guest_reads_its_input_in_pieces_and_gets_zero_at_the_end() {
        let (fs, caps) = setup();
        let wat = r#"(module
          (import "iyakki" "stdin_read" (func $rd (param i32 i32) (result i32)))
          (import "iyakki" "log" (func $log (param i32 i32)))
          (memory (export "memory") 1)
          (func (export "main") (result i32) (local $n i32) (local $total i32)
            (block $done (loop $again
              (local.set $n (call $rd (i32.const 0) (i32.const 4)))
              (br_if $done (i32.eqz (local.get $n)))
              (call $log (i32.const 0) (local.get $n))
              (local.set $total (i32.add (local.get $total) (local.get $n)))
              (br $again)))
            (local.get $total)))"#;
        let sb = Sandbox::new(SandboxConfig::default()).unwrap().with_stdin(b"hello world\n".to_vec());
        let out = sb.run(wat.as_bytes(), "main", fs.clone(), caps.clone(), vec![]).unwrap();
        assert_eq!(out.log, b"hello world\n");
        assert_eq!(out.result, Some(12));
        // no input given: the first read is already the end
        let none = Sandbox::new(SandboxConfig::default()).unwrap();
        assert_eq!(none.run(wat.as_bytes(), "main", fs, caps, vec![]).unwrap().result, Some(0));
    }

    #[test]
    fn a_cancel_flag_really_stops_a_running_guest_and_is_told_apart_from_the_time_limit() {
        let (fs, caps) = setup();
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let sb = Sandbox::new(SandboxConfig { fuel: u64::MAX / 2, ..Default::default() }).unwrap().with_cancel(flag.clone());
        let wat = r#"(module (func (export "main") (loop br 0)))"#;
        let f2 = flag.clone();
        let t = std::time::Instant::now();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            f2.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        assert!(matches!(sb.run(wat.as_bytes(), "main", fs.clone(), caps.clone(), vec![]), Err(WasmError::Cancelled)));
        assert!(t.elapsed() < std::time::Duration::from_secs(5), "the loop was interrupted, not waited out");
        // a flag that is never raised changes nothing
        let calm = Sandbox::new(SandboxConfig::default()).unwrap().with_cancel(Arc::new(std::sync::atomic::AtomicBool::new(false)));
        let quick = r#"(module (func (export "main") (result i32) i32.const 7))"#;
        assert_eq!(calm.run(quick.as_bytes(), "main", fs, caps, vec![]).unwrap().result, Some(7));
    }

    #[test]
    fn wall_time_stops_a_guest_that_fuel_would_let_run() {
        let (fs, caps) = setup();
        let sb = Sandbox::new(SandboxConfig { fuel: u64::MAX / 2, ..Default::default() }).unwrap().with_wall_time(std::time::Duration::from_millis(100));
        let wat = r#"(module (func (export "main") (loop br 0)))"#;
        let t = std::time::Instant::now();
        assert!(matches!(sb.run(wat.as_bytes(), "main", fs.clone(), caps.clone(), vec![]), Err(WasmError::Timeout)));
        assert!(t.elapsed() < std::time::Duration::from_secs(5));
        // a quick guest is unaffected, and the limit does not linger after the call
        let ok = Sandbox::new(SandboxConfig::default()).unwrap().with_wall_time(std::time::Duration::from_millis(100));
        let quick = r#"(module (func (export "main") (result i32) i32.const 1))"#;
        let mut s = ok.session(quick.as_bytes(), fs, caps, vec![]).unwrap();
        assert_eq!(s.call("main", &[]).unwrap().result, Some(1));
        std::thread::sleep(std::time::Duration::from_millis(250));
        assert_eq!(s.call("main", &[]).unwrap().result, Some(1));
    }

    #[test]
    fn memory_growth_is_capped() {
        let (fs, caps) = setup();
        let sb = Sandbox::new(SandboxConfig { max_memory_bytes: 2 * 65536, ..Default::default() }).unwrap();
        // memory.grow returns -1 on failure
        let wat = r#"(module (memory 1) (func (export "main") (result i32) i32.const 10 memory.grow))"#;
        let out = sb.run(wat.as_bytes(), "main", fs.clone(), caps.clone(), vec![]).unwrap();
        assert_eq!(out.result, Some(-1));
        // Declaring more than the cap fails to instantiate.
        let big = r#"(module (memory 10) (func (export "main")))"#;
        assert!(sb.run(big.as_bytes(), "main", fs, caps, vec![]).is_err());
    }

    /// Guest writes "hi" to /w/out.txt using token #0, returns the status code.
    const WRITER: &str = r#"
    (module
      (import "iyakki" "token_get" (func $tg (param i32 i32) (result i32)))
      (import "iyakki" "fs_write" (func $fw (param i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (data (i32.const 64) "/w/out.txt")
      (data (i32.const 128) "hi")
      (func (export "main") (result i32)
        (drop (call $tg (i32.const 0) (i32.const 0)))
        (call $fw (i32.const 0) (i32.const 64) (i32.const 10) (i32.const 128) (i32.const 2))))
    "#;

    #[test]
    fn write_with_valid_token() {
        let (fs, caps) = setup();
        let t = caps.issue(Capability::FsWrite("/w".into())).unwrap();
        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let out = sb.run(WRITER.as_bytes(), "main", fs.clone(), caps, vec![t]).unwrap();
        assert_eq!(out.result, Some(0));
        assert_eq!(fs.lock().unwrap().read("/w/out.txt").unwrap(), b"hi");
    }

    #[test]
    fn write_denied_outside_scope_or_without_grant() {
        let (fs, caps) = setup();
        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        // Token scoped elsewhere.
        let t = caps.issue(Capability::FsWrite("/other".into())).unwrap();
        let out = sb.run(WRITER.as_bytes(), "main", fs.clone(), caps.clone(), vec![t]).unwrap();
        assert_eq!(out.result, Some(errno::DENIED));
        // Read-only token.
        let t = caps.issue(Capability::FsRead("/w".into())).unwrap();
        let out = sb.run(WRITER.as_bytes(), "main", fs.clone(), caps.clone(), vec![t]).unwrap();
        assert_eq!(out.result, Some(errno::DENIED));
        // No token granted: token_get fails, memory holds zeros -> unknown token -> denied.
        let out = sb.run(WRITER.as_bytes(), "main", fs.clone(), caps, vec![]).unwrap();
        assert_eq!(out.result, Some(errno::DENIED));
        assert!(!fs.lock().unwrap().exists("/w/out.txt"));
    }

    #[test]
    fn read_roundtrip_and_log() {
        let (fs, caps) = setup();
        fs.lock().unwrap().write("/w/in.txt", b"abc").unwrap();
        let t = caps.issue(Capability::FsRead("/w".into())).unwrap();
        let wat = r#"
        (module
          (import "iyakki" "token_get" (func $tg (param i32 i32) (result i32)))
          (import "iyakki" "fs_read" (func $fr (param i32 i32 i32 i32 i32) (result i32)))
          (import "iyakki" "log" (func $log (param i32 i32)))
          (memory (export "memory") 1)
          (data (i32.const 64) "/w/in.txt")
          (func (export "main") (result i32)
            (local $n i32)
            (drop (call $tg (i32.const 0) (i32.const 0)))
            (local.set $n (call $fr (i32.const 0) (i32.const 64) (i32.const 9) (i32.const 256) (i32.const 16)))
            (call $log (i32.const 256) (local.get $n))
            (local.get $n)))
        "#;
        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let out = sb.run(wat.as_bytes(), "main", fs, caps, vec![t]).unwrap();
        assert_eq!(out.result, Some(3));
        assert_eq!(out.log, b"abc");
    }

    #[test]
    fn path_traversal_rejected() {
        let (fs, caps) = setup();
        fs.lock().unwrap().write("/secret", b"x").unwrap();
        let t = caps.issue(Capability::FsWrite("/w".into())).unwrap();
        let wat = WRITER.replace("/w/out.txt", "/w/../secret");
        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let out = sb.run(wat.as_bytes(), "main", fs.clone(), caps, vec![t]).unwrap();
        assert_eq!(out.result, Some(errno::DENIED));
        assert_eq!(fs.lock().unwrap().read("/secret").unwrap(), b"x");
    }

    const COUNTER: &str = r#"
    (module
      (memory (export "memory") 1)
      (global $g (export "g") (mut i32) (i32.const 0))
      (func (export "inc") (param i32) (result i32)
        (i32.store (i32.const 0) (local.get 0))
        (global.set $g (i32.add (global.get $g) (local.get 0)))
        (global.get $g)))
    "#;

    fn counter() -> Session {
        let (fs, caps) = setup();
        Sandbox::new(SandboxConfig::default()).unwrap().session(COUNTER.as_bytes(), fs, caps, vec![]).unwrap()
    }

    #[test]
    fn snapshot_restore_roundtrip() {
        let mut s = counter();
        assert_eq!(s.call("inc", &[5]).unwrap().result, Some(5));
        let snap = s.snapshot();
        assert_eq!(s.call("inc", &[3]).unwrap().result, Some(8));
        s.restore(&snap).unwrap();
        assert_eq!(s.call("inc", &[0]).unwrap().result, Some(5));
        // serialized form survives
        let snap2 = Snapshot::from_bytes(&snap.to_bytes(), 1 << 20).unwrap();
        assert_eq!(snap2.memory, snap.memory);
        assert_eq!(snap2.globals, snap.globals);
        assert!(Snapshot::from_bytes(b"junk", 1 << 20).is_none());
    }

    #[test]
    fn fork_is_independent() {
        let mut s = counter();
        s.call("inc", &[5]).unwrap();
        s.vfs().lock().unwrap().write("/a", b"1").unwrap();
        let mut f = s.fork().unwrap();
        assert_eq!(f.call("inc", &[100]).unwrap().result, Some(105));
        f.vfs().lock().unwrap().write("/a", b"2").unwrap();
        assert_eq!(s.call("inc", &[0]).unwrap().result, Some(5));
        assert_eq!(s.vfs().lock().unwrap().read("/a").unwrap(), b"1");
    }

    fn big_guest(pages: u32) -> Session {
        let wat = format!(
            r#"(module (memory (export "memory") {pages})
              (global $g (export "g") (mut i32) (i32.const 0))
              (func (export "poke") (param i32) (result i32)
                local.get 0 i32.const 7 i32.store8 local.get 0)
              (func (export "peek") (param i32) (result i32) local.get 0 i32.load8_u))"#
        );
        let (fs, caps) = setup();
        let cfg = SandboxConfig { max_memory_bytes: 1 << 30, ..SandboxConfig::default() };
        Sandbox::new(cfg).unwrap().session(wat.as_bytes(), fs, caps, vec![]).unwrap()
    }

    #[test]
    fn paged_snapshots_share_unchanged_pages_and_restore_writes_only_changed_ones() {
        let mut s = big_guest(16);
        s.call("poke", &[100]).unwrap();
        s.call("poke", &[PAGE as i32 * 3 + 5]).unwrap();
        let a = s.snapshot_paged(None);
        assert_eq!(a.page_count(), 16);
        assert_eq!(a.memory(), s.memory_bytes());
        // one more page changes: the child shares the other fifteen with the parent
        s.call("poke", &[PAGE as i32 * 9]).unwrap();
        let b = s.snapshot_paged(Some(&a));
        assert_eq!(b.shared_pages_with(&a), 15);
        assert_eq!(b.memory(), s.memory_bytes());
        // restoring the old snapshot writes exactly the one page that differs
        assert_eq!(s.restore_paged(&a).unwrap(), 1);
        assert_eq!(s.memory_bytes(), a.memory());
        assert_eq!(s.call("peek", &[PAGE as i32 * 9]).unwrap().result, Some(0));
        assert_eq!(s.call("peek", &[100]).unwrap().result, Some(7));
        // restoring what is already there writes nothing
        assert_eq!(s.restore_paged(&a).unwrap(), 0);
        // plain snapshot of the same state is identical
        assert_eq!(a.to_snapshot().memory, s.snapshot().memory);
    }

    #[test]
    fn a_fork_from_a_paged_snapshot_and_a_direct_fork_both_start_at_the_parents_state_and_stay_apart() {
        let mut s = big_guest(4);
        s.call("poke", &[PAGE as i32 + 1]).unwrap();
        let snap = s.snapshot_paged(None);
        let mut f1 = s.fork_from_paged(&snap).unwrap();
        let mut f2 = s.fork().unwrap();
        assert_eq!(f1.call("peek", &[PAGE as i32 + 1]).unwrap().result, Some(7));
        assert_eq!(f2.call("peek", &[PAGE as i32 + 1]).unwrap().result, Some(7));
        f1.call("poke", &[5]).unwrap();
        assert_eq!(f2.call("peek", &[5]).unwrap().result, Some(0));
        assert_eq!(s.call("peek", &[5]).unwrap().result, Some(0));
        assert_eq!(f1.memory_bytes().len(), s.memory_bytes().len());
    }

    /// Real timing of fork and paged snapshot at 1, 16 and 64 MiB. Run by hand:
    /// `cargo test -p iyakki-wasm fork_timing -- --ignored --nocapture`
    #[test]
    #[ignore = "benchmark: prints numbers, asserts nothing about speed"]
    fn fork_timing() {
        for mib in [1u32, 16, 64] {
            let mut s = big_guest(mib * 16);
            for i in 0..(mib * 16) {
                s.call("poke", &[(i as usize * PAGE + 17) as i32]).unwrap();
            }
            let med = |mut v: Vec<u128>| {
                v.sort();
                v[v.len() / 2]
            };
            let mut fk = Vec::new();
            let mut sp = Vec::new();
            let mut sp2 = Vec::new();
            let mut rp = Vec::new();
            let mut ff = Vec::new();
            let base = s.snapshot_paged(None);
            for _ in 0..15 {
                let t = std::time::Instant::now();
                let f = s.fork().unwrap();
                fk.push(t.elapsed().as_micros());
                drop(f);
                let t = std::time::Instant::now();
                let p = s.snapshot_paged(None);
                sp.push(t.elapsed().as_micros());
                drop(p);
                let t = std::time::Instant::now();
                let p = s.snapshot_paged(Some(&base));
                sp2.push(t.elapsed().as_micros());
                drop(p);
                let t = std::time::Instant::now();
                s.restore_paged(&base).unwrap();
                rp.push(t.elapsed().as_micros());
                let t = std::time::Instant::now();
                let f = s.fork_from_paged(&base).unwrap();
                ff.push(t.elapsed().as_micros());
                drop(f);
            }
            println!(
                "FORKTIME {mib} MiB: fork {} us, snapshot_paged {} us, snapshot_paged(with parent) {} us, restore_paged(unchanged) {} us, fork_from_paged {} us",
                med(fk), med(sp), med(sp2), med(rp), med(ff)
            );
        }
    }


    // ---- F-WASM-14: copy-on-write forks -------------------------------------------------------------------------------------------

    fn binary_guest(wat_text: &str, max_bytes: usize) -> Session {
        let wasm = wat::parse_str(wat_text).unwrap();
        let (fs, caps) = setup();
        let cfg = SandboxConfig { max_memory_bytes: max_bytes, ..SandboxConfig::default() };
        Sandbox::new(cfg).unwrap().session(&wasm, fs, caps, vec![]).unwrap()
    }

    fn big_binary(pages: u32) -> Session {
        binary_guest(
            &format!(
                r#"(module (memory (export "memory") {pages})
                  (global $g (export "g") (mut i32) (i32.const 0))
                  (func (export "setg") (param i32) local.get 0 global.set $g)
                  (func (export "getg") (result i32) global.get $g)
                  (func (export "grow") (param i32) (result i32) local.get 0 memory.grow)
                  (func (export "poke") (param i32) (result i32) local.get 0 i32.const 7 i32.store8 local.get 0)
                  (func (export "peek") (param i32) (result i32) local.get 0 i32.load8_u))"#
            ),
            1 << 30,
        )
    }

    #[test]
    fn a_cow_fork_starts_at_the_parents_state_and_every_fork_stays_apart() {
        let mut s = big_binary(16);
        s.call("poke", &[100]).unwrap();
        s.call("poke", &[(PAGE * 9 + 5) as i32]).unwrap();
        s.call("setg", &[42]).unwrap();
        let image = s.cow_image().unwrap();
        // the parent goes on after the image was made; the image keeps the state of that moment
        s.call("poke", &[200]).unwrap();
        let mut a = image.fork().unwrap();
        let mut b = image.fork().unwrap();
        for f in [&mut a, &mut b] {
            assert_eq!(f.call("peek", &[100]).unwrap().result, Some(7));
            assert_eq!(f.call("peek", &[(PAGE * 9 + 5) as i32]).unwrap().result, Some(7));
            assert_eq!(f.call("peek", &[200]).unwrap().result, Some(0), "state of the moment of the image, not of the later parent");
            assert_eq!(f.call("getg", &[]).unwrap().result, Some(42));
        }
        a.call("poke", &[300]).unwrap();
        a.call("setg", &[1]).unwrap();
        assert_eq!(b.call("peek", &[300]).unwrap().result, Some(0), "a write in one fork is not seen by the other");
        assert_eq!(b.call("getg", &[]).unwrap().result, Some(42));
        assert_eq!(s.call("peek", &[300]).unwrap().result, Some(0), "nor by the parent");
        assert_eq!(a.memory_bytes().len(), 16 * PAGE);
    }

    #[test]
    fn a_cow_image_of_a_guest_that_grew_and_a_fork_that_grows_again_work() {
        let mut s = big_binary(2);
        s.call("grow", &[3]).unwrap();
        s.call("poke", &[(PAGE * 4 + 1) as i32]).unwrap();
        let image = s.cow_image().unwrap();
        let mut f = image.fork().unwrap();
        assert_eq!(f.memory_bytes().len(), 5 * PAGE);
        assert_eq!(f.call("peek", &[(PAGE * 4 + 1) as i32]).unwrap().result, Some(7));
        f.call("grow", &[1]).unwrap();
        assert_eq!(f.memory_bytes().len(), 6 * PAGE);
        // an image of a fork works too (the rewritten module is rewritten again)
        let mut again = f.cow_image().unwrap().fork().unwrap();
        assert_eq!(again.memory_bytes().len(), 6 * PAGE);
    }

    #[test]
    fn the_rewrite_keeps_other_data_and_drops_the_start_function_and_refuses_what_it_cannot_keep() {
        // active data and a start function that already ran: the snapshot has both, the image runs no start again
        let mut s = binary_guest(
            r#"(module (memory (export "memory") 1) (data (i32.const 11) "abc")
                 (global $n (export "n") (mut i32) (i32.const 0))
                 (func $start global.get $n i32.const 1 i32.add global.set $n i32.const 50 i32.const 9 i32.store8)
                 (start $start)
                 (func (export "peek") (param i32) (result i32) local.get 0 i32.load8_u)
                 (func (export "getn") (result i32) global.get $n))"#,
            1 << 24,
        );
        assert_eq!(s.call("getn", &[]).unwrap().result, Some(1));
        let mut f = s.cow_image().unwrap().fork().unwrap();
        assert_eq!(f.call("peek", &[11]).unwrap().result, Some(b'a' as i32), "the data segment is in the image");
        assert_eq!(f.call("peek", &[50]).unwrap().result, Some(9), "what the start function wrote is in the image");
        assert_eq!(f.call("getn", &[]).unwrap().result, Some(1), "the start function did not run a second time");
        // WAT text, a passive data segment and an imported memory are refused with a reason
        let (fs, caps) = setup();
        let mut text = Sandbox::new(SandboxConfig::default()).unwrap().session(b"(module (memory (export \"memory\") 1))", fs, caps, vec![]).unwrap();
        assert!(matches!(text.cow_image(), Err(WasmError::Load(m)) if m.contains("WAT")));
        let mut passive = binary_guest(r#"(module (memory (export "memory") 1) (data "xyz"))"#, 1 << 24);
        assert!(matches!(passive.cow_image(), Err(WasmError::Load(m)) if m.contains("passive")));
        let wasm = wat::parse_str(r#"(module (import "env" "m" (memory 1)))"#).unwrap();
        assert!(cow::rewrite(&wasm, &vec![0u8; PAGE]).unwrap_err().contains("imported"));
        assert!(cow::rewrite(b"\0asm\x01\0\0\0", &[1, 2, 3]).is_err());
    }

    /// Fork cost with and without the copy, at 1, 16 and 64 MiB. Run by hand:
    /// `cargo test -p iyakki-wasm cow_fork_timing -- --ignored --nocapture`
    #[test]
    #[ignore = "benchmark: prints numbers, asserts nothing about speed"]
    fn cow_fork_timing() {
        for mib in [1u32, 16, 64] {
            let mut s = big_binary(mib * 16);
            for i in 0..(mib * 16) {
                s.call("poke", &[(i as usize * PAGE + 17) as i32]).unwrap();
            }
            let med = |mut v: Vec<u128>| {
                v.sort();
                v[v.len() / 2]
            };
            let (mut copy, mut make, mut cow, mut touch) = (vec![], vec![], vec![], vec![]);
            let mut image = None;
            for _ in 0..15 {
                let t = std::time::Instant::now();
                let f = s.fork().unwrap();
                copy.push(t.elapsed().as_micros());
                drop(f);
                let t = std::time::Instant::now();
                let i = s.cow_image().unwrap();
                make.push(t.elapsed().as_micros());
                image = Some(i);
            }
            let image = image.unwrap();
            for _ in 0..15 {
                let t = std::time::Instant::now();
                let mut f = image.fork().unwrap();
                cow.push(t.elapsed().as_micros());
                let t = std::time::Instant::now();
                f.call("poke", &[5]).unwrap();
                touch.push(t.elapsed().as_micros());
            }
            println!("COWTIME {mib} MiB: fork (copy) {} us, make image {} us, fork from image {} us, first write in a fork {} us", med(copy), med(make), med(cow), med(touch));
        }
    }

    #[test]
    fn many_branches_of_a_big_guest_start_from_an_image_and_give_the_same_answer_as_copies() {
        assert!(!image_pays(23, 64 << 20) && !image_pays(100, 1 << 20) && image_pays(24, 16 << 20));
        let mut s = big_binary(256); // 16 MiB
        s.call("setg", &[9]).unwrap();
        let (win, results) = speculate(&mut s, COW_FORKS, "poke", |r, _| r.outcome.as_ref().unwrap().result.unwrap() as f64).unwrap().unwrap();
        assert_eq!((win, results.len()), (COW_FORKS - 1, COW_FORKS));
        // the winner's write is the parent's now; the losers' writes are not
        assert_eq!(s.call("peek", &[(COW_FORKS - 1) as i32]).unwrap().result, Some(7));
        assert_eq!(s.call("peek", &[5]).unwrap().result, Some(0));
        assert_eq!(s.call("getg", &[]).unwrap().result, Some(9));
        // forks from the image and by copy are the same state
        let a = s.fork_many(COW_FORKS).unwrap();
        let b = s.fork_many(2).unwrap();
        assert_eq!((a.len(), b.len()), (COW_FORKS, 2));
        let (mut a0, mut b0) = (a.into_iter().next().unwrap(), b.into_iter().next().unwrap());
        assert_eq!(a0.memory_bytes(), b0.memory_bytes());
        assert_eq!(a0.call("getg", &[]).unwrap().result, b0.call("getg", &[]).unwrap().result);
    }

    #[test]
    fn a_guest_that_cannot_have_an_image_is_still_forked_many_times_by_copy() {
        let mut s = counter();
        let v = s.fork_many(COW_FORKS + 1).unwrap();
        assert_eq!(v.len(), COW_FORKS + 1);
    }

    #[test]
    fn speculation_commits_best_branch() {
        let mut s = counter();
        s.call("inc", &[1]).unwrap();
        // branch i calls inc(i): results 1,2,3,4; branch 3 scores highest.
        let (win, results) = speculate(&mut s, 4, "inc", |r, _| r.outcome.as_ref().unwrap().result.unwrap() as f64)
            .unwrap()
            .unwrap();
        assert_eq!((win, results.len()), (3, 4));
        assert_eq!(s.call("inc", &[0]).unwrap().result, Some(4));
    }

    #[test]
    fn speculation_tie_goes_to_lowest_index_and_failures_lose() {
        let mut s = counter();
        let (win, _) = speculate(&mut s, 3, "inc", |_, _| 1.0).unwrap().unwrap();
        assert_eq!(win, 0);
        // entry with wrong arity fails on every branch -> no winner, parent untouched
        let none = speculate(&mut s, 2, "nope", |_, _| 1.0).unwrap();
        assert!(none.is_none());
    }

    #[test]
    fn ledger_records_and_time_travels() {
        let mut s = counter();
        s.start_recording();
        let init = s.memory_bytes();
        s.record_input(b"prompt");
        s.call("inc", &[7]).unwrap();
        s.call("inc", &[9]).unwrap();
        let l = s.take_ledger().unwrap();
        l.verify().unwrap();
        assert_eq!(l.calls().count(), 2);
        assert_eq!(l.memory_at(&init, 1).unwrap()[0], 7);
        assert_eq!(l.memory_at(&init, 2).unwrap()[0], 9);
    }

    #[test]
    fn ledger_records_host_calls() {
        let (fs, caps) = setup();
        let t = caps.issue(Capability::FsWrite("/other".into())).unwrap();
        let mut s = Sandbox::new(SandboxConfig::default()).unwrap().session(WRITER.as_bytes(), fs, caps, vec![t]).unwrap();
        s.start_recording();
        s.call("main", &[]).unwrap();
        let l = s.take_ledger().unwrap();
        assert!(l.entries.iter().any(|e| matches!(&e.event,
            Event::HostCall { name, status, .. } if name == "fs_write" && *status == errno::DENIED)));
    }

    struct Echo;
    impl NetBackend for Echo {
        fn fetch(&self, m: &str, u: &str, b: &[u8]) -> Result<Vec<u8>, i32> {
            Ok(format!("{m} {u} {}", b.len()).into_bytes())
        }
    }

    const FETCH: &str = r#"
    (module
      (import "iyakki" "token_get" (func $tg (param i32 i32) (result i32)))
      (import "iyakki" "http_fetch" (func $hf (param i32 i32 i32 i32 i32 i32 i32) (result i32)))
      (import "iyakki" "log" (func $log (param i32 i32)))
      (memory (export "memory") 1)
      (data (i32.const 64) "https://api.example.com/v1")
      (func (export "main") (result i32)
        (local $n i32)
        (drop (call $tg (i32.const 0) (i32.const 0)))
        (local.set $n (call $hf (i32.const 0) (i32.const 64) (i32.const 26) (i32.const 0) (i32.const 0) (i32.const 512) (i32.const 128)))
        (if (i32.gt_s (local.get $n) (i32.const 0)) (then (call $log (i32.const 512) (local.get $n))))
        (local.get $n)))
    "#;

    #[test]
    fn http_fetch_policy_and_backend() {
        let (fs, caps) = setup();
        let sb = Sandbox::new(SandboxConfig::default()).unwrap();
        let ok = caps.issue(Capability::NetConnect("api.example.com".into())).unwrap();
        let bad = caps.issue(Capability::NetConnect("other.com".into())).unwrap();
        // no backend configured
        let o = sb.run(FETCH.as_bytes(), "main", fs.clone(), caps.clone(), vec![ok]).unwrap();
        assert_eq!(o.result, Some(errno::NET));
        let sb = sb.with_net(Arc::new(Echo));
        let o = sb.run(FETCH.as_bytes(), "main", fs.clone(), caps.clone(), vec![ok]).unwrap();
        assert_eq!(o.log, b"GET https://api.example.com/v1 0");
        // token for a different host is denied before reaching the backend
        let o = sb.run(FETCH.as_bytes(), "main", fs, caps, vec![bad]).unwrap();
        assert_eq!(o.result, Some(errno::DENIED));
    }
}
