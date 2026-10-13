//! The engine: instantiate the WebAssembly build of QuickJS-ng, give it the host functions, run scripts under limits.

use std::any::Any;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::cache;
use crate::sha256::Sha256;
use wasmtime::{Caller, Config, Engine, Extern, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder, TypedFunc};

/// The engine build, committed so that ordinary builds need no C compiler (see `dist/js/build.sh`).
const ENGINE_WASM: &[u8] = include_bytes!("../assets/iyakki-js.wasm");

#[derive(Clone, Debug)]
pub struct Limits {
    /// Memory the whole engine may use.
    pub memory_bytes: usize,
    /// An instruction budget for one call (a hard stop for runaway loops). It is counted in the engine's own
    /// interrupt ticks (one per 10 000 loop turns, calls or regular-expression steps; a tick is taken to be 150 000
    /// WebAssembly instructions), not by metering every instruction, which costs every script a good part of its speed.
    pub fuel: u64,
    /// Wall-clock time one call may take.
    pub wall: Duration,
    /// Script call-stack depth, in bytes.
    pub stack_bytes: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { memory_bytes: 320 << 20, fuel: 12_000_000_000, wall: Duration::from_secs(20), stack_bytes: 1 << 20 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JsError {
    /// The script threw (message and stack).
    Exception(String),
    /// It ran out of its instruction budget.
    OutOfFuel,
    /// It ran past its time limit.
    Timeout,
    /// Something in the sandbox went wrong (a trap, memory exhaustion).
    Sandbox(String),
}

impl std::fmt::Display for JsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JsError::Exception(m) => write!(f, "{m}"),
            JsError::OutOfFuel => write!(f, "the script ran too long (instruction limit)"),
            JsError::Timeout => write!(f, "the script ran too long (time limit)"),
            JsError::Sandbox(m) => write!(f, "sandbox: {m}"),
        }
    }
}

/// A piece of the web platform for a page's scripts.
pub trait WebModule: Send {
    /// The name scripts use to reach this module (`__host("name", op, json)`).
    fn name(&self) -> &'static str;
    /// JavaScript that runs once before the page's scripts and defines this module's globals.
    fn prelude(&self) -> &'static str {
        ""
    }
    /// Answer a call from the prelude (or any script).
    fn call(&mut self, op: &str, args: &serde_json::Value) -> Result<serde_json::Value, String>;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

struct Host {
    modules: Vec<Box<dyn WebModule>>,
    result: Vec<u8>,
    deadline: Instant,
    /// interrupt ticks left in the instruction budget
    ticks_left: u64,
    out_of_fuel: bool,
    limits: StoreLimits,
    stderr: Vec<u8>,
    /// per `module.op`: calls and time (kept only when `IYAKKI_JS_STATS` is set, for profiling)
    stats: Option<std::collections::BTreeMap<String, (u64, Duration)>>,
}

pub struct JsEngine {
    store: Store<Host>,
    memory: Memory,
    alloc: TypedFunc<u32, u32>,
    free: TypedFunc<u32, ()>,
    eval_module_fn: TypedFunc<(u32, u32, u32, u32), i32>,
    module_status_fn: TypedFunc<(), i32>,
    eval_fn: TypedFunc<(u32, u32, u32, u32), i32>,
    jobs_fn: TypedFunc<(), i32>,
    out_fn: TypedFunc<(), u32>,
    out_len_fn: TypedFunc<(), u32>,
    mem_used_fn: TypedFunc<(), u64>,
    compile_fn: TypedFunc<(u32, u32, u32, u32), i32>,
    run_pending_fn: TypedFunc<(), i32>,
    eval_bytecode_fn: TypedFunc<(u32, u32), i32>,
    profile_fn: TypedFunc<i32, ()>,
    limits: Limits,
}

/// Epoch ticks that cover `wall` plus the slack.
fn epoch_ticks(wall: Duration) -> u64 {
    (wall.as_millis().min(1 << 40) as u64 + EPOCH_SLACK_MS) / EPOCH_MS + 2
}

/// Wasm instructions the budget counts per interrupt tick (see [`Limits::fuel`]).
const FUEL_PER_TICK: u64 = 150_000;
/// How often the watchdog advances the engine's epoch. A trap on the epoch is only the backstop behind the script
/// engine's own interrupt check (every 10 000 operations), for code that never reaches that check.
const EPOCH_MS: u64 = 50;
/// Slack between the wall limit and the epoch trap.
const EPOCH_SLACK_MS: u64 = 1_000;

fn config() -> Config {
    let mut c = Config::new();
    // a time limit by epoch interruption: far cheaper than metering every instruction with fuel
    c.epoch_interruption(true);
    c.cranelift_opt_level(wasmtime::OptLevel::Speed);
    c.cranelift_nan_canonicalization(false);
    // deep script recursion must hit QuickJS's own limit (an exception), not the sandbox's stack (a trap)
    c.async_stack_size(6 << 20);
    c.max_wasm_stack(4 << 20);
    c
}

static LAST_ARM_MS: AtomicU64 = AtomicU64::new(0);
static MAX_WALL_MS: AtomicU64 = AtomicU64::new(0);
static WATCHDOG: OnceLock<std::thread::Thread> = OnceLock::new();

fn now_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Advance the epoch while some engine has been armed lately, and sleep otherwise.
fn start_watchdog(engine: Engine) {
    let Ok(h) = std::thread::Builder::new().name("iyakki-js-watchdog".into()).spawn(move || loop {
        let idle_after = MAX_WALL_MS.load(Ordering::Relaxed) + EPOCH_SLACK_MS + 5_000;
        if now_ms() > LAST_ARM_MS.load(Ordering::Relaxed) + idle_after {
            std::thread::park();
        } else {
            std::thread::sleep(Duration::from_millis(EPOCH_MS));
            engine.increment_epoch();
        }
    }) else {
        return;
    };
    let _ = WATCHDOG.set(h.thread().clone());
}

/// Compiling the engine takes a moment; once per process is enough (and once per machine, see [`cached_module`]).
fn shared() -> Result<&'static (Engine, Module), String> {
    static E: OnceLock<Result<(Engine, Module), String>> = OnceLock::new();
    E.get_or_init(|| {
        let engine = Engine::new(&config()).map_err(|e| e.to_string())?;
        let module = cached_module(&engine)?;
        start_watchdog(engine.clone());
        Ok((engine, module))
    })
    .as_ref()
    .map_err(|e| e.clone())
}

/// Scripts at least this long keep their compiled form on disk ([`JsEngine::eval_cached`]).
const SCRIPT_CACHE_MIN: usize = 128 << 10;
/// The compiled scripts may take this much disk before the oldest go.
const SCRIPT_CACHE_MAX: u64 = 512 << 20;

/// A short digest naming what the cached engine was built from: the engine, wasmtime's version and our settings.
fn engine_digest(engine: &Engine) -> &'static str {
    static D: OnceLock<String> = OnceLock::new();
    D.get_or_init(|| {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        engine.precompile_compatibility_hash().hash(&mut h);
        let mut s = Sha256::new();
        s.update(ENGINE_WASM);
        s.update(&h.finish().to_le_bytes());
        crate::sha256::hex(&s.finish()[..16])
    })
}

/// The compiled engine from the per-user cache directory, or compiled now and stored there (see [`crate::cache`]).
/// wasmtime refuses a serialized module from another version or configuration, which is also a miss.
fn cached_module(engine: &Engine) -> Result<Module, String> {
    let compile = || Module::new(engine, ENGINE_WASM).map_err(|e| e.to_string());
    let Some(dir) = cache::dir() else { return compile() };
    let path = dir.join(format!("engine-{}.cwasm", engine_digest(engine)));
    if let Some(bytes) = cache::read(&path) {
        // SAFETY: the bytes come from a file only we can write, with a matching checksum, that this program
        // stored from `Module::serialize` of the same engine build; wasmtime also checks version and settings.
        if let Ok(m) = unsafe { Module::deserialize(engine, &bytes) } {
            return Ok(m);
        }
    }
    let module = compile()?;
    if let Ok(bytes) = module.serialize() {
        cache::write(&path, &bytes);
        // builds of the engine come and go; keep the last few
        cache::prune(&dir, "cwasm", 24 << 20);
    }
    Ok(module)
}

/// Compile the engine in the background so the first page with scripts does not wait for it.
pub fn warm_up() {
    let _ = std::thread::Builder::new().name("iyakki-js-warmup".into()).stack_size(16 << 20).spawn(|| {
        let _ = shared();
    });
}

const MAX_RESULT: usize = 16 << 20;

fn memory_of(c: &mut Caller<'_, Host>) -> Option<Memory> {
    match c.get_export("memory") {
        Some(Extern::Memory(m)) => Some(m),
        _ => None,
    }
}

fn read_str(c: &mut Caller<'_, Host>, mem: Memory, ptr: i32, len: i32) -> Option<String> {
    let (ptr, len) = (usize::try_from(ptr).ok()?, usize::try_from(len).ok()?);
    let data = mem.data(&*c);
    String::from_utf8(data.get(ptr..ptr.checked_add(len)?)?.to_vec()).ok()
}

impl JsEngine {
    pub fn new(limits: Limits, modules: Vec<Box<dyn WebModule>>) -> Result<JsEngine, String> {
        let (engine, module) = shared()?;
        let mut linker: Linker<Host> = Linker::new(engine);
        linker
            .func_wrap("iyakki", "host_call", |mut c: Caller<'_, Host>, mp: i32, ml: i32, op: i32, ol: i32, ap: i32, al: i32| -> i32 {
                let Some(mem) = memory_of(&mut c) else { return 0 };
                let (Some(m), Some(o), Some(a)) = (read_str(&mut c, mem, mp, ml), read_str(&mut c, mem, op, ol), read_str(&mut c, mem, ap, al)) else {
                    c.data_mut().result = b"bad host call".to_vec();
                    return -13;
                };
                let started = Instant::now();
                let outcome = (|| {
                    let args: serde_json::Value = if a.is_empty() { serde_json::Value::Null } else { serde_json::from_str(&a).map_err(|e| format!("bad arguments: {e}"))? };
                    let host = c.data_mut();
                    let module = host.modules.iter_mut().find(|x| x.name() == m).ok_or_else(|| format!("no module called {m}"))?;
                    module.call(&o, &args)
                })();
                let host = c.data_mut();
                if let Some(st) = host.stats.as_mut() {
                    let e = st.entry(format!("{m}.{o}")).or_default();
                    e.0 += 1;
                    e.1 += started.elapsed();
                }
                // time spent waiting on the host (a network request) is not the script's time
                host.deadline += started.elapsed();
                let remaining = host.deadline.saturating_duration_since(Instant::now());
                wasmtime::AsContextMut::as_context_mut(&mut c).set_epoch_deadline(epoch_ticks(remaining));
                let host = c.data_mut();
                match outcome {
                    Ok(v) => {
                        // the module loader wants the text itself, not a JSON string
                        let s = match (&v, m.as_str()) {
                            (serde_json::Value::String(t), "module") => t.clone(),
                            _ => serde_json::to_string(&v).unwrap_or_else(|_| "null".into()),
                        };
                        if s.len() > MAX_RESULT {
                            host.result = b"result too large".to_vec();
                            return -16;
                        }
                        host.result = s.into_bytes();
                        host.result.len() as i32
                    }
                    Err(e) => {
                        let mut e = e.into_bytes();
                        if e.is_empty() {
                            e = b"error".to_vec();
                        }
                        e.truncate(4096);
                        let n = e.len() as i32;
                        host.result = e;
                        -n
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        linker
            .func_wrap("iyakki", "host_result", |mut c: Caller<'_, Host>, out: i32, cap: i32| {
                let Some(mem) = memory_of(&mut c) else { return };
                let data = std::mem::take(&mut c.data_mut().result);
                let n = data.len().min(cap.max(0) as usize);
                if let Ok(out) = usize::try_from(out) {
                    let _ = mem.write(&mut c, out, &data[..n]);
                }
                c.data_mut().result = data;
            })
            .map_err(|e| e.to_string())?;
        // also stop the script (with its stack in the error) while a little fuel is left, instead of a bare trap at zero
        linker
            .func_wrap("iyakki", "interrupt", |mut c: Caller<'_, Host>| -> i32 {
                let h = c.data_mut();
                h.ticks_left = h.ticks_left.saturating_sub(1);
                if h.ticks_left == 0 {
                    h.out_of_fuel = true;
                    return 1;
                }
                (Instant::now() > h.deadline) as i32
            })
            .map_err(|e| e.to_string())?;
        // The little of WASI that the C library touches: time, randomness, and a place for error text. No files, no network.
        let wasi = "wasi_snapshot_preview1";
        linker
            .func_wrap(wasi, "clock_time_get", |mut c: Caller<'_, Host>, id: i32, _precision: i64, out: i32| -> i32 {
                let Some(mem) = memory_of(&mut c) else { return 8 };
                let ns: u64 = match id {
                    0 => std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0),
                    _ => {
                        static START: OnceLock<Instant> = OnceLock::new();
                        START.get_or_init(Instant::now).elapsed().as_nanos() as u64 + 1_000_000
                    }
                };
                if mem.write(&mut c, out as usize, &ns.to_le_bytes()).is_err() { 21 } else { 0 }
            })
            .map_err(|e| e.to_string())?;
        linker
            .func_wrap(wasi, "random_get", |mut c: Caller<'_, Host>, ptr: i32, len: i32| -> i32 {
                let Some(mem) = memory_of(&mut c) else { return 8 };
                let mut x = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1) | 1;
                let buf: Vec<u8> = (0..len.max(0)).map(|_| { x ^= x << 13; x ^= x >> 7; x ^= x << 17; (x >> 24) as u8 }).collect();
                if mem.write(&mut c, ptr as usize, &buf).is_err() { 21 } else { 0 }
            })
            .map_err(|e| e.to_string())?;
        linker
            .func_wrap(wasi, "fd_write", |mut c: Caller<'_, Host>, fd: i32, iovs: i32, n: i32, written: i32| -> i32 {
                let Some(mem) = memory_of(&mut c) else { return 8 };
                let mut total = 0u32;
                let mut collected = Vec::new();
                for i in 0..n.max(0) as usize {
                    let base = iovs as usize + i * 8;
                    let mut b = [0u8; 8];
                    if mem.read(&c, base, &mut b).is_err() {
                        return 21;
                    }
                    let (p, l) = (u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize, u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize);
                    let mut chunk = vec![0u8; l.min(1 << 16)];
                    if mem.read(&c, p, &mut chunk).is_err() {
                        return 21;
                    }
                    total += chunk.len() as u32;
                    collected.extend_from_slice(&chunk);
                }
                if fd == 2 || fd == 1 {
                    let host = c.data_mut();
                    if host.stderr.len() < 8192 {
                        host.stderr.extend_from_slice(&collected);
                    }
                }
                let _ = mem.write(&mut c, written as usize, &total.to_le_bytes());
                0
            })
            .map_err(|e| e.to_string())?;
        // anything else the C library might import traps if it is ever called
        linker.define_unknown_imports_as_traps(module).map_err(|e| e.to_string())?;

        let store_limits = StoreLimitsBuilder::new().memory_size(limits.memory_bytes).instances(1).memories(1).build();
        let mut store = Store::new(engine, Host { modules, result: Vec::new(), deadline: Instant::now() + limits.wall, ticks_left: u64::MAX, out_of_fuel: false, limits: store_limits, stderr: Vec::new(), stats: std::env::var_os("IYAKKI_JS_STATS").map(|_| Default::default()) });
        store.limiter(|h| &mut h.limits);
        store.set_epoch_deadline(u64::MAX / 2);
        let instance = linker.instantiate(&mut store, module).map_err(|e| format!("could not start the JavaScript engine: {e}"))?;
        let memory = instance.get_memory(&mut store, "memory").ok_or("the engine has no memory")?;
        if let Ok(init) = instance.get_typed_func::<(), ()>(&mut store, "_initialize") {
            init.call(&mut store, ()).map_err(|e| e.to_string())?;
        }
        let get = |name: &str| format!("the engine lacks {name}");
        let mut e = JsEngine {
            alloc: instance.get_typed_func(&mut store, "jb_alloc").map_err(|_| get("jb_alloc"))?,
            free: instance.get_typed_func(&mut store, "jb_free").map_err(|_| get("jb_free"))?,
            eval_fn: instance.get_typed_func(&mut store, "jb_eval").map_err(|_| get("jb_eval"))?,
            eval_module_fn: instance.get_typed_func(&mut store, "jb_eval_module").map_err(|_| get("jb_eval_module"))?,
            module_status_fn: instance.get_typed_func(&mut store, "jb_module_status").map_err(|_| get("jb_module_status"))?,
            jobs_fn: instance.get_typed_func(&mut store, "jb_jobs").map_err(|_| get("jb_jobs"))?,
            out_fn: instance.get_typed_func(&mut store, "jb_out").map_err(|_| get("jb_out"))?,
            out_len_fn: instance.get_typed_func(&mut store, "jb_out_len").map_err(|_| get("jb_out_len"))?,
            mem_used_fn: instance.get_typed_func(&mut store, "jb_memory_used").map_err(|_| get("jb_memory_used"))?,
            compile_fn: instance.get_typed_func(&mut store, "jb_compile").map_err(|_| get("jb_compile"))?,
            run_pending_fn: instance.get_typed_func(&mut store, "jb_run_pending").map_err(|_| get("jb_run_pending"))?,
            eval_bytecode_fn: instance.get_typed_func(&mut store, "jb_eval_bytecode").map_err(|_| get("jb_eval_bytecode"))?,
            profile_fn: instance.get_typed_func(&mut store, "jb_profile").map_err(|_| get("jb_profile"))?,
            store,
            memory,
            limits,
        };
        let init: TypedFunc<(u32, u32), i32> = instance.get_typed_func(&mut e.store, "jb_init").map_err(|_| get("jb_init"))?;
        e.arm();
        let mem_limit = (e.limits.memory_bytes / 4 * 3) as u32;
        let rc = init.call(&mut e.store, (mem_limit, e.limits.stack_bytes)).map_err(|t| format!("engine start-up failed: {t}"))?;
        if rc != 0 {
            return Err(format!("engine start-up failed ({rc})"));
        }
        // each module's globals
        let preludes: Vec<(&'static str, &'static str)> = e.store.data().modules.iter().map(|m| (m.name(), m.prelude())).collect();
        for (name, src) in preludes {
            if !src.is_empty() {
                e.eval(&format!("<{name}>"), src).map_err(|err| format!("the {name} module failed to start: {err}"))?;
            }
        }
        Ok(e)
    }

    /// What the engine wrote to its error stream (where a stopped script was, for instance).
    pub fn last_stderr(&self) -> String {
        String::from_utf8_lossy(&self.store.data().stderr).trim().to_string()
    }

    /// Give the next call its full budget.
    fn arm(&mut self) {
        let wall = self.limits.wall;
        let h = self.store.data_mut();
        h.deadline = Instant::now() + wall;
        h.ticks_left = (self.limits.fuel / FUEL_PER_TICK).max(1);
        h.out_of_fuel = false;
        self.store.set_epoch_deadline(epoch_ticks(wall));
        MAX_WALL_MS.fetch_max(wall.as_millis().min(u64::MAX as u128 / 2) as u64, Ordering::Relaxed);
        LAST_ARM_MS.store(now_ms(), Ordering::Relaxed);
        if let Some(w) = WATCHDOG.get() {
            w.unpark();
        }
    }

    /// The script was stopped by the engine's interrupt check: which limit?
    fn interrupted(&self) -> JsError {
        if self.store.data().out_of_fuel { JsError::OutOfFuel } else { JsError::Timeout }
    }

    fn classify(&mut self, trap: wasmtime::Error) -> JsError {
        let text = format!("{trap:#}");
        if text.contains("interrupt") {
            // the epoch backstop: the script got past its time limit without ever reaching the engine's own check, and
            // was cut off in the middle of whatever it was doing, so the engine cannot be used again
            JsError::Sandbox("the script ran past its time limit and was cut off".into())
        } else if text.contains("memory") && text.contains("limit") || text.contains("failed to grow") {
            JsError::Sandbox("out of memory".into())
        } else {
            let e = String::from_utf8_lossy(&self.store.data().stderr).trim().to_string();
            JsError::Sandbox(if e.is_empty() { text.lines().next().unwrap_or("trap").to_string() } else { format!("{} ({e})", text.lines().next().unwrap_or("trap")) })
        }
    }

    fn read_out(&mut self) -> String {
        let (Ok(ptr), Ok(len)) = (self.out_fn.call(&mut self.store, ()), self.out_len_fn.call(&mut self.store, ())) else { return String::new() };
        let data = self.memory.data(&self.store);
        String::from_utf8_lossy(data.get(ptr as usize..ptr as usize + len as usize).unwrap_or(&[])).into_owned()
    }

    fn put(&mut self, bytes: &[u8]) -> Result<u32, JsError> {
        let ptr = self.alloc.call(&mut self.store, bytes.len() as u32).map_err(|t| self.classify(t))?;
        if ptr == 0 {
            return Err(JsError::Sandbox("out of memory".into()));
        }
        self.memory.write(&mut self.store, ptr as usize, bytes).map_err(|e| JsError::Sandbox(e.to_string()))?;
        Ok(ptr)
    }

    /// Run a script in the global scope; the value of its last expression, as text.
    pub fn eval(&mut self, name: &str, source: &str) -> Result<String, JsError> {
        self.arm();
        // the engine wants both the script and its name to end with a zero byte (not counted in the length)
        let mut src = source.as_bytes().to_vec();
        src.push(0);
        let s = self.put(&src)?;
        let mut nm = name.as_bytes().to_vec();
        nm.push(0);
        let n = self.put(&nm)?;
        let rc = self.eval_fn.call(&mut self.store, (s, source.len() as u32, n, nm.len() as u32 - 1));
        let _ = self.free.call(&mut self.store, s);
        let _ = self.free.call(&mut self.store, n);
        let rc = rc.map_err(|t| self.classify(t))?;
        self.finish_script(rc)
    }

    /// What a finished script call left: its value as text, or why it failed.
    fn finish_script(&mut self, rc: i32) -> Result<String, JsError> {
        let out = self.read_out();
        if rc == 0 {
            Ok(out)
        } else if out.contains("interrupted") {
            Err(self.interrupted())
        } else {
            Err(JsError::Exception(out))
        }
    }

    /// Like [`JsEngine::eval`], but a big script's compiled form is kept on disk, so the next time the same script is
    /// run (the same page again, another page using the same file) it is not parsed again. Parsing is the main cost of
    /// a megabyte-sized bundle. The file is named by a SHA-256 of the engine build, the script's name and its text.
    pub fn eval_cached(&mut self, name: &str, source: &str) -> Result<String, JsError> {
        let dir = if source.len() >= SCRIPT_CACHE_MIN { cache::dir() } else { None };
        let Some(dir) = dir else { return self.eval(name, source) };
        let digest = match shared() {
            Ok((e, _)) => engine_digest(e),
            Err(_) => return self.eval(name, source),
        };
        let mut sha = Sha256::new();
        sha.update(digest.as_bytes());
        sha.update(&(name.len() as u64).to_le_bytes());
        sha.update(name.as_bytes());
        sha.update(source.as_bytes());
        let path = dir.join(format!("script-{}.qbc", crate::sha256::hex(&sha.finish())));
        if let Some(code) = cache::read(&path) {
            self.arm();
            let p = self.put(&code)?;
            let rc = self.eval_bytecode_fn.call(&mut self.store, (p, code.len() as u32));
            let _ = self.free.call(&mut self.store, p);
            let rc = rc.map_err(|t| self.classify(t))?;
            if rc != 2 {
                return self.finish_script(rc);
            }
            // unreadable (an engine from before a format change that kept its name): parse the text
        }
        self.arm();
        let mut src = source.as_bytes().to_vec();
        src.push(0);
        let s = self.put(&src)?;
        let mut nm = name.as_bytes().to_vec();
        nm.push(0);
        let n = self.put(&nm)?;
        let rc = self.compile_fn.call(&mut self.store, (s, source.len() as u32, n, nm.len() as u32 - 1));
        let _ = self.free.call(&mut self.store, s);
        let _ = self.free.call(&mut self.store, n);
        let rc = rc.map_err(|t| self.classify(t))?;
        if rc != 0 {
            return self.finish_script(rc);
        }
        let code = self.read_out_bytes();
        cache::write(&path, &code);
        cache::prune(&dir, "qbc", SCRIPT_CACHE_MAX);
        self.arm();
        let rc = self.run_pending_fn.call(&mut self.store, ()).map_err(|t| self.classify(t))?;
        self.finish_script(rc)
    }

    /// Start sampling where the scripts spend their time (a development aid, `IYAKKI_JS_PROFILE`): every interrupt tick
    /// the engine hands the script stack to a counter. [`JsEngine::profile_report`] says what it found.
    pub fn profile_start(&mut self) {
        if let Err(e) = self.eval(
            "profiler",
            r#"(() => {
              const self = new Map(), incl = new Map(); let total = 0;
              const clean = (f) => f.trim().replace(/^at /, '').replace(/:\d+\)$/, ')').replace(/https?:\/\/[^ )]*\//, '');
              globalThis.__prof = (st) => {
                const fr = String(st).split('\n').slice(1).map(clean).filter(Boolean);
                total++;
                if (fr.length) self.set(fr[0], (self.get(fr[0]) || 0) + 1);
                const seen = new Set();
                for (const f of fr) { const k = f.replace(/:\d+\)$/, ')'); if (!seen.has(k)) { seen.add(k); incl.set(k, (incl.get(k) || 0) + 1); } }
              };
              globalThis.__profdump = () => {
                const top = (m) => [...m].sort((a, b) => b[1] - a[1]).slice(0, 25).map(([k, v]) => (100 * v / total).toFixed(1).padStart(5) + '%  ' + k.slice(0, 150));
                return 'samples ' + total + '\nSELF\n' + top(self).join('\n') + '\nINCLUSIVE\n' + top(incl).join('\n');
              };
            })()"#,
        ) {
            eprintln!("the profiler could not start: {e}");
            return;
        }
        let _ = self.profile_fn.call(&mut self.store, 1);
    }

    pub fn profile_report(&mut self) -> String {
        let _ = self.profile_fn.call(&mut self.store, 0);
        self.eval("profile report", "typeof __profdump === 'function' ? __profdump() : ''").unwrap_or_default()
    }

    fn read_out_bytes(&mut self) -> Vec<u8> {
        let (Ok(ptr), Ok(len)) = (self.out_fn.call(&mut self.store, ()), self.out_len_fn.call(&mut self.store, ())) else { return Vec::new() };
        self.memory.data(&self.store).get(ptr as usize..ptr as usize + len as usize).unwrap_or(&[]).to_vec()
    }

    /// Run an ES module: `name` is its address (relative imports resolve against it through the `module` plug-in). The
    /// module's imports are fetched, it runs, and any top-level `await` is waited for as far as the queued jobs go.
    pub fn eval_module(&mut self, name: &str, source: &str) -> Result<(), JsError> {
        self.arm();
        let mut src = source.as_bytes().to_vec();
        src.push(0);
        let s = self.put(&src)?;
        let mut nm = name.as_bytes().to_vec();
        nm.push(0);
        let n = self.put(&nm)?;
        let rc = self.eval_module_fn.call(&mut self.store, (s, source.len() as u32, n, nm.len() as u32 - 1));
        let _ = self.free.call(&mut self.store, s);
        let _ = self.free.call(&mut self.store, n);
        let rc = rc.map_err(|t| self.classify(t))?;
        if rc != 0 {
            let out = self.read_out();
            if out.contains("interrupted") { if std::env::var("IYAKKI_JS_DEBUG").is_ok() { eprintln!("JSDBG {}", String::from_utf8_lossy(&self.store.data().stderr)); } return Err(self.interrupted()); } return Err(JsError::Exception(out));
        }
        self.run_jobs()?;
        match self.module_status_fn.call(&mut self.store, ()).map_err(|t| self.classify(t))? {
            1 => Err(JsError::Exception(self.read_out())),
            _ => Ok(()),
        }
    }

    /// Run the queued promise reactions (microtasks). How many ran.
    pub fn run_jobs(&mut self) -> Result<u32, JsError> {
        self.arm();
        let n = self.jobs_fn.call(&mut self.store, ()).map_err(|t| self.classify(t))?;
        if n < 0 {
            let out = self.read_out();
            if out.contains("interrupted") { if std::env::var("IYAKKI_JS_DEBUG").is_ok() { eprintln!("JSDBG {}", String::from_utf8_lossy(&self.store.data().stderr)); } return Err(self.interrupted()); } return Err(JsError::Exception(out));
        }
        Ok(n as u32)
    }

    /// Host-call counts and times by `module.op`, when `IYAKKI_JS_STATS` is set (profiling aid).
    pub fn host_stats(&self) -> Vec<(String, u64, Duration)> {
        let mut v: Vec<_> = self.store.data().stats.iter().flatten().map(|(k, (n, t))| (k.clone(), *n, *t)).collect();
        v.sort_by_key(|x| std::cmp::Reverse(x.2));
        v
    }

    /// Bytes the engine's heap uses now.
    pub fn memory_used(&mut self) -> u64 {
        self.mem_used_fn.call(&mut self.store, ()).unwrap_or(0)
    }

    /// A module, to read what it collected (the console's lines, a storage's contents...).
    pub fn module<T: 'static>(&mut self, name: &str) -> Option<&mut T> {
        self.store.data_mut().modules.iter_mut().find(|m| m.name() == name).and_then(|m| m.as_any_mut().downcast_mut::<T>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo(Vec<String>);
    impl WebModule for Echo {
        fn name(&self) -> &'static str {
            "echo"
        }
        fn prelude(&self) -> &'static str {
            "globalThis.echo = (x) => JSON.parse(__host('echo', 'say', JSON.stringify({ x })));"
        }
        fn call(&mut self, op: &str, args: &serde_json::Value) -> Result<serde_json::Value, String> {
            match op {
                "say" => {
                    self.0.push(args["x"].to_string());
                    Ok(serde_json::json!({ "you_said": args["x"], "n": self.0.len() }))
                }
                "fail" => Err("it failed".into()),
                other => Err(format!("unknown op {other}")),
            }
        }
        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
    }

    fn engine() -> JsEngine {
        JsEngine::new(Limits::default(), vec![Box::new(Echo(vec![]))]).unwrap()
    }

    #[test]
    fn javascript_runs_and_values_come_back_as_text() {
        let mut e = engine();
        assert_eq!(e.eval("t", "1 + 2 * 3").unwrap(), "7");
        assert_eq!(e.eval("t", "[1,2,3].map(x => x * 2).join('-')").unwrap(), "2-4-6");
        assert_eq!(e.eval("t", "var greet = n => `hi ${n}`; greet('web')").unwrap(), "hi web");
        assert_eq!(e.eval("t", "greet('again')").unwrap(), "hi again", "state is kept between scripts");
        assert_eq!(e.eval("t", "void 0").unwrap(), "");
        // modern language features
        assert_eq!(e.eval("t", "class A { #x = 4; get x() { return this.#x; } } new A().x + (null ?? 1) + [...'ab'].length").unwrap(), "7");
        assert_eq!(e.eval("t", "JSON.stringify(Object.fromEntries(new Map([['a', 1]])))").unwrap(), "{\"a\":1}");
    }

    #[test]
    fn exceptions_carry_their_message_and_the_engine_survives() {
        let mut e = engine();
        let err = e.eval("page.js", "function f() { throw new TypeError('bad thing'); } f()").unwrap_err();
        let JsError::Exception(m) = err else { panic!("{err:?}") };
        assert!(m.contains("TypeError: bad thing") && m.contains("page.js"), "{m}");
        assert!(matches!(e.eval("t", "this is not javascript"), Err(JsError::Exception(m)) if m.contains("SyntaxError")));
        assert_eq!(e.eval("t", "40 + 2").unwrap(), "42", "still alive");
    }

    #[test]
    fn promises_run_when_the_host_asks() {
        let mut e = engine();
        e.eval("t", "var log = []; Promise.resolve(1).then(v => { log.push('then ' + v); return v + 1; }).then(v => log.push('then ' + v)); log.push('sync')").unwrap();
        assert_eq!(e.eval("t", "log.join(',')").unwrap(), "sync");
        assert!(e.run_jobs().unwrap() >= 2);
        assert_eq!(e.eval("t", "log.join(',')").unwrap(), "sync,then 1,then 2");
        e.eval("t", "(async () => { await null; log.push('async'); })()").unwrap();
        e.run_jobs().unwrap();
        assert!(e.eval("t", "log.join(',')").unwrap().ends_with(",async"));
    }

    #[test]
    fn a_module_is_called_through_the_host_and_errors_come_back_as_exceptions() {
        let mut e = engine();
        assert_eq!(e.eval("t", "echo('hello').n").unwrap(), "1");
        assert_eq!(e.eval("t", "echo({ a: [1, 2] }).you_said.a[1]").unwrap(), "2");
        assert!(matches!(e.eval("t", "__host('echo', 'fail', '{}')"), Err(JsError::Exception(m)) if m.contains("it failed")));
        assert!(matches!(e.eval("t", "__host('nothing', 'x', '{}')"), Err(JsError::Exception(m)) if m.contains("no module called nothing")));
        assert!(matches!(e.eval("t", "__host('echo', 'say', '{not json')"), Err(JsError::Exception(m)) if m.contains("bad arguments")));
        let echo = e.module::<Echo>("echo").unwrap();
        assert_eq!(echo.0.len(), 2);
    }

    #[test]
    fn a_script_has_no_way_out_of_the_sandbox() {
        let mut e = engine();
        // none of the host's powers exist unless a module provides them
        for probe in ["typeof fetch", "typeof require", "typeof process", "typeof std", "typeof os", "typeof window", "typeof document", "typeof XMLHttpRequest", "typeof localStorage", "typeof setTimeout"] {
            assert_eq!(e.eval("t", probe).unwrap(), "undefined", "{probe}");
        }
    }

    #[test]
    fn an_endless_loop_is_stopped_by_the_time_limit_or_the_fuel() {
        let mut e = JsEngine::new(Limits { wall: Duration::from_millis(300), ..Limits::default() }, vec![]).unwrap();
        // on a busy machine the instruction budget can run out just before the clock does; either way it is stopped
        let stopped = e.eval("t", "while (true) {}").unwrap_err();
        assert!(matches!(stopped, JsError::Timeout | JsError::OutOfFuel), "{stopped:?}");
        assert_eq!(e.eval("t", "1 + 1").unwrap(), "2", "the engine is usable afterwards");
        let mut f = JsEngine::new(Limits { fuel: 200_000_000, wall: Duration::from_secs(60), ..Limits::default() }, vec![]).unwrap();
        assert_eq!(f.eval("t", "let i = 0; while (true) { i++; }").unwrap_err(), JsError::OutOfFuel);
    }

    #[test]
    fn memory_hogs_and_deep_recursion_fail_cleanly() {
        let mut e = JsEngine::new(Limits { memory_bytes: 32 << 20, ..Limits::default() }, vec![]).unwrap();
        let r = e.eval("t", "const a = []; while (true) a.push(new Array(10000).fill('x'.repeat(100)));");
        assert!(matches!(r, Err(JsError::Exception(ref m)) if m.contains("out of memory") || m.contains("InternalError")) || matches!(r, Err(JsError::Sandbox(_))) || matches!(r, Err(JsError::Timeout)), "{r:?}");
        let mut e = engine();
        let r = e.eval("t", "function r(n) { return r(n + 1) + 1; } r(0)");
        assert!(matches!(r, Err(JsError::Exception(ref m)) if m.contains("stack") || m.contains("RangeError") || m.contains("InternalError")), "{r:?}");
        assert_eq!(e.eval("t", "'ok'").unwrap(), "ok");
    }

    #[test]
    fn engines_are_independent() {
        let mut a = engine();
        let mut b = engine();
        a.eval("t", "var secret = 'a-only'").unwrap();
        assert_eq!(b.eval("t", "typeof secret").unwrap(), "undefined");
    }

    fn qbc_files() -> Vec<std::path::PathBuf> {
        let Some(dir) = cache::dir() else { return Vec::new() };
        std::fs::read_dir(dir).map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "qbc")).collect()).unwrap_or_default()
    }

    #[test]
    fn a_big_script_is_parsed_once_and_runs_from_its_compiled_form_after_that() {
        if cache::dir().is_none() {
            return; // no safe place for a cache here
        }
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        // over the size where caching starts; the last line throws, so the position in the error shows the name and line
        let body = format!("/* {stamp} {} */\nvar total = 0;\nfor (var i = 0; i < 10; i++) total += i;\nif (total !== 45) throw new Error('wrong');\ntotal + 1000", "x".repeat(SCRIPT_CACHE_MIN));
        let boom = format!("{}\nfunction f() {{\n  throw new TypeError('bad {stamp}');\n}}\nf();", "// pad ".repeat(SCRIPT_CACHE_MIN / 7 + 10));
        let before = qbc_files();
        let mut a = engine();
        assert_eq!(a.eval_cached("big.js", &body).unwrap(), "1045");
        let new: Vec<_> = qbc_files().into_iter().filter(|p| !before.contains(p)).collect();
        assert_eq!(new.len(), 1, "the compiled script is stored: {new:?}");
        let stored = std::fs::metadata(&new[0]).unwrap().modified().unwrap();
        // another engine: same answer, and the stored file is used, not rewritten
        let mut b = engine();
        assert_eq!(b.eval_cached("big.js", &body).unwrap(), "1045");
        assert_eq!(std::fs::metadata(&new[0]).unwrap().modified().unwrap(), stored);
        assert_eq!(b.eval("t", "total").unwrap(), "45", "it ran in the global scope");
        // a different name is a different script (the name is part of the bytecode)
        assert_eq!(b.eval_cached("other.js", &body).unwrap(), "1045");
        assert_eq!(qbc_files().into_iter().filter(|p| !before.contains(p)).count(), 2);
        // errors name the file and line the same way from the cache as from the source
        let first = engine().eval_cached("boom.js", &boom).unwrap_err();
        let second = engine().eval_cached("boom.js", &boom).unwrap_err();
        let (JsError::Exception(m1), JsError::Exception(m2)) = (&first, &second) else { panic!("{first:?} {second:?}") };
        assert!(m1.contains("bad ") && m1.contains("boom.js:3"), "{m1}");
        assert_eq!(m1, m2);
        // a syntax error is reported and not stored
        let n = qbc_files().len();
        assert!(matches!(engine().eval_cached("bad.js", &format!("{} this is not javascript", " ".repeat(SCRIPT_CACHE_MIN))), Err(JsError::Exception(m)) if m.contains("SyntaxError")));
        assert_eq!(qbc_files().len(), n);
        // a stored file that does not hold bytecode (a damaged one that kept its checksum) falls back to the text
        cache::write(&new[0], b"definitely not bytecode");
        assert_eq!(engine().eval_cached("big.js", &body).unwrap(), "1045");
        // small scripts are not stored
        let n = qbc_files().len();
        assert_eq!(engine().eval_cached("small.js", "6 * 7").unwrap(), "42");
        assert_eq!(qbc_files().len(), n);
        for p in qbc_files().into_iter().filter(|p| !before.contains(p)) {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn the_instruction_budget_counts_work_not_time_and_resets_for_each_call() {
        // 5 000 000 loop turns are about 500 ticks: a budget for 2 000 ticks lets one pass and not ten times that
        let lim = Limits { fuel: 2_000 * FUEL_PER_TICK, wall: Duration::from_secs(60), ..Limits::default() };
        let mut e = JsEngine::new(lim, vec![]).unwrap();
        assert_eq!(e.eval("t", "let s = 0; for (let i = 0; i < 5000000; i++) s += i; s").unwrap(), "12499997500000");
        assert_eq!(e.eval("t", "s = 0; for (let i = 0; i < 5000000; i++) s += i; s").unwrap(), "12499997500000", "each call has its own budget");
        assert_eq!(e.eval("t", "for (let i = 0; i < 50000000; i++) s += i;").unwrap_err(), JsError::OutOfFuel);
        assert_eq!(e.eval("t", "1 + 1").unwrap(), "2", "and the engine is fine afterwards");
        // a regular expression that backtracks forever is stopped too
        let mut r = JsEngine::new(Limits { wall: Duration::from_millis(400), ..Limits::default() }, vec![]).unwrap();
        let stopped = r.eval("t", "/(a+)+$/.test('a'.repeat(60) + 'b')").unwrap_err();
        assert!(matches!(stopped, JsError::Timeout | JsError::OutOfFuel), "{stopped:?}");
        assert_eq!(r.eval("t", "2 + 2").unwrap(), "4");
    }
}
