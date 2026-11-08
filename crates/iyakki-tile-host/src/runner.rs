use iyakki_sec::CapabilityStore;
use iyakki_tile_abi::{limits, DecodeError, DisplayList, Event, EventBatch, ABI_VERSION};
use iyakki_vfs::MemFs;
use iyakki_wasm::{Sandbox, SandboxConfig, Session, WasmError};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What the app manifest declares about a tile (the `tile` section of `iyakki-app.json`).
#[derive(Clone, Debug, Default)]
pub struct TileManifest {
    pub name: String,
    /// The ABI version the app was built for; must equal `ABI_VERSION`.
    pub abi: u32,
    /// Service capabilities: `"domain"`, `"domain.*"` or `"domain.method"`.
    pub caps: Vec<String>,
}

impl TileManifest {
    pub fn new(name: &str, caps: &[&str]) -> Self {
        TileManifest { name: name.into(), abi: ABI_VERSION, caps: caps.iter().map(|s| s.to_string()).collect() }
    }
    pub fn allows(&self, domain: &str, method: &str) -> bool {
        self.caps.iter().any(|c| {
            c == domain || *c == format!("{domain}.*") || *c == format!("{domain}.{method}")
        })
    }
}

/// Services reached through `call(domain, method, json) -> json`. The host implements this on top of
/// the real services; the capability check happens before it is called.
pub trait ServiceBackend: Send + Sync {
    fn call(&self, domain: &str, method: &str, json: &str) -> Result<String, String>;
}

struct NoServices;
impl ServiceBackend for NoServices {
    fn call(&self, _: &str, _: &str, _: &str) -> Result<String, String> {
        Err("no services".into())
    }
}

/// Per-call budgets. The hard stop is the largest fuel value plus the wall limit; each call kind is
/// also checked against its own fuel budget afterwards.
#[derive(Clone, Debug)]
pub struct Budgets {
    pub init_fuel: u64,
    pub event_fuel: u64,
    pub frame_fuel: u64,
    pub wall: Duration,
    pub max_memory: usize,
    /// Host import calls (`call`, `measure_text`) allowed in one init/event/frame call.
    pub max_host_calls: u32,
}

impl Default for Budgets {
    fn default() -> Self {
        Budgets {
            init_fuel: 20_000_000,
            event_fuel: 5_000_000,
            frame_fuel: 10_000_000,
            wall: Duration::from_millis(100),
            max_memory: 16 * 1024 * 1024,
            max_host_calls: 256,
        }
    }
}

/// Why a tile was refused or stopped. `Display` is plain language for the person.
#[derive(Debug)]
pub enum TileError {
    VersionMismatch { tile: u32, host: u32 },
    MissingExport(String),
    Load(String),
    Budget(&'static str),
    Trap(String),
    TileFailed(i32),
    BadPointer,
    BadDisplayList(DecodeError),
    OutOfBounds { op: usize },
    BadEvents(DecodeError),
}

impl std::fmt::Display for TileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TileError::VersionMismatch { tile, host } => write!(
                f,
                "This app was built for tile interface version {tile}, but this system speaks version {host}. Update the app or the system."
            ),
            TileError::MissingExport(n) => write!(f, "This is not a tile: the required function `{n}` is missing."),
            TileError::Load(m) => write!(f, "The app could not be loaded: {m}"),
            TileError::Budget(w) => write!(f, "The app was stopped: it used too much {w} for one step."),
            TileError::Trap(m) => write!(f, "The app crashed: {m}"),
            TileError::TileFailed(c) => write!(f, "The app reported an error (code {c})."),
            TileError::BadPointer => write!(f, "The app returned data from outside its own memory."),
            TileError::BadDisplayList(e) => write!(f, "The app returned a bad picture: {e}."),
            TileError::OutOfBounds { op } => write!(f, "The app tried to draw far outside its window (drawing step {op})."),
            TileError::BadEvents(e) => write!(f, "The events for the app were bad: {e}."),
        }
    }
}
impl std::error::Error for TileError {}

/// Cost of one step, for measurement and logs.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameStats {
    pub fuel_used: u64,
    pub host_time: Duration,
    pub list_bytes: usize,
}

#[derive(Clone, Copy)]
enum Kind {
    Init,
    Event,
    Frame,
}

pub struct TileRunner {
    session: Session,
    budgets: Budgets,
    manifest: TileManifest,
    host_calls: Arc<AtomicU32>,
    size: (i32, i32),
    last: Option<DisplayList>,
    pub last_stats: FrameStats,
    pub log: Vec<u8>,
}

fn rd(mem: &[u8], ptr: i32, len: i32) -> Option<&[u8]> {
    let (p, l) = (u32::try_from(ptr).ok()? as usize, u32::try_from(len).ok()? as usize);
    mem.get(p..p.checked_add(l)?)
}

impl TileRunner {
    /// Load a tile (`.wasm` bytes or WAT text). Refuses a wrong ABI version or missing exports.
    pub fn load(
        wasm: &[u8],
        manifest: TileManifest,
        budgets: Budgets,
        backend: Option<Arc<dyn ServiceBackend>>,
        rng_seed: u64,
    ) -> Result<TileRunner, TileError> {
        if manifest.abi != ABI_VERSION {
            return Err(TileError::VersionMismatch { tile: manifest.abi, host: ABI_VERSION });
        }
        let fuel = budgets.init_fuel.max(budgets.event_fuel).max(budgets.frame_fuel);
        let host_calls = Arc::new(AtomicU32::new(0));
        let backend: Arc<dyn ServiceBackend> = backend.unwrap_or_else(|| Arc::new(NoServices));
        let rng = Arc::new(AtomicU64::new(rng_seed | 1));
        let max_calls = budgets.max_host_calls;

        let hc = host_calls.clone();
        let measure: iyakki_wasm::HostImportFn = Arc::new(move |mem, a| {
            if hc.fetch_add(1, Ordering::Relaxed) >= max_calls {
                return -8;
            }
            match rd(mem, a[2], a[3]).and_then(|b| std::str::from_utf8(b).ok()) {
                Some(t) if a[1] > 0 && (a[1] as u32) <= limits::MAX_FONT_SIZE => {
                    iyakki_tile_abi::measure_text(a[0] as u8, a[1] as u32, t)
                }
                _ => -1,
            }
        });
        let hc = host_calls.clone();
        let m2 = manifest.clone();
        let call: iyakki_wasm::HostImportFn = Arc::new(move |mem, a| {
            if hc.fetch_add(1, Ordering::Relaxed) >= max_calls {
                return -8;
            }
            let text = |p, l| rd(mem, p, l).and_then(|b| std::str::from_utf8(b).ok()).map(str::to_owned);
            let (Some(dom), Some(meth), Some(json)) = (text(a[0], a[1]), text(a[2], a[3]), text(a[4], a[5])) else {
                return iyakki_wasm::errno::BAD_MEMORY;
            };
            if json.len() > 64 * 1024 {
                return iyakki_wasm::errno::BUF_TOO_SMALL;
            }
            if !m2.allows(&dom, &meth) {
                return iyakki_wasm::errno::DENIED;
            }
            let Ok(out) = backend.call(&dom, &meth, &json) else { return iyakki_wasm::errno::IO };
            let (Ok(op), Ok(cap)) = (usize::try_from(a[6]), usize::try_from(a[7])) else {
                return iyakki_wasm::errno::BAD_MEMORY;
            };
            if out.len() > cap {
                return iyakki_wasm::errno::BUF_TOO_SMALL;
            }
            match op.checked_add(out.len()).and_then(|e| mem.get_mut(op..e)) {
                Some(dst) => {
                    dst.copy_from_slice(out.as_bytes());
                    out.len() as i32
                }
                None => iyakki_wasm::errno::BAD_MEMORY,
            }
        });
        let random: iyakki_wasm::HostImportFn = Arc::new(move |_, _| {
            let mut s = rng.load(Ordering::Relaxed);
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            rng.store(s, Ordering::Relaxed);
            (s >> 16) as i32
        });

        let sb = Sandbox::new(SandboxConfig { fuel, max_memory_bytes: budgets.max_memory })
            .map_err(|e| TileError::Load(e.to_string()))?
            .with_wall_time(budgets.wall)
            .with_import("iyakki_tile", "measure_text", 4, measure)
            .with_import("iyakki_tile", "call", 8, call)
            .with_import("iyakki_tile", "random_u32", 0, random);
        let session = sb
            .session(wasm, Arc::new(Mutex::new(MemFs::new())), CapabilityStore::new(), vec![])
            .map_err(|e| TileError::Load(e.to_string()))?;
        let mut r = TileRunner {
            session,
            budgets,
            manifest,
            host_calls,
            size: (320, 200),
            last: None,
            last_stats: FrameStats::default(),
            log: Vec::new(),
        };
        let tile_abi = r.call_raw("iyakki_tile_abi", &[], Kind::Init)?.unwrap_or(-1) as u32;
        if tile_abi != ABI_VERSION {
            return Err(TileError::VersionMismatch { tile: tile_abi, host: ABI_VERSION });
        }
        Ok(r)
    }

    pub fn manifest(&self) -> &TileManifest {
        &self.manifest
    }

    fn call_raw(&mut self, name: &str, args: &[i32], kind: Kind) -> Result<Option<i32>, TileError> {
        self.host_calls.store(0, Ordering::Relaxed);
        let out = self.session.call(name, args).map_err(|e| match e {
            WasmError::OutOfFuel => TileError::Budget("computing time"),
            WasmError::Timeout => TileError::Budget("real time"),
            WasmError::NoEntry(n) => TileError::MissingExport(n),
            WasmError::Trap(m) => TileError::Trap(m),
            WasmError::Load(m) => TileError::Load(m),
        })?;
        let budget = match kind {
            Kind::Init => self.budgets.init_fuel,
            Kind::Event => self.budgets.event_fuel,
            Kind::Frame => self.budgets.frame_fuel,
        };
        self.last_stats.fuel_used = out.fuel_used;
        if self.log.len() < 1 << 16 {
            self.log.extend_from_slice(&out.log);
        }
        if out.fuel_used > budget {
            return Err(TileError::Budget("computing time"));
        }
        Ok(out.result)
    }

    /// Copy `bytes` into guest memory obtained from `iyakki_alloc`; returns the guest pointer.
    fn put(&mut self, bytes: &[u8]) -> Result<i32, TileError> {
        let ptr = self
            .call_raw("iyakki_alloc", &[bytes.len() as i32], Kind::Event)?
            .ok_or_else(|| TileError::MissingExport("iyakki_alloc".into()))?;
        if ptr <= 0 {
            return Err(TileError::BadPointer);
        }
        self.session.write_memory(ptr as u32, bytes).map_err(|_| TileError::BadPointer)?;
        Ok(ptr)
    }

    /// Start the tile with the surface size and theme. Call once before events or frames.
    pub fn init(&mut self, w: i32, h: i32, scale_pct: u32, theme: Event) -> Result<(), TileError> {
        self.size = (w, h);
        let bytes = EventBatch::new(vec![Event::Resize { w, h, scale_pct }, theme])
            .encode()
            .map_err(TileError::BadEvents)?;
        let ptr = self.put(&bytes)?;
        let t = Instant::now();
        let rc = self.call_raw("iyakki_tile_init", &[ABI_VERSION as i32, ptr, bytes.len() as i32], Kind::Init)?;
        self.last_stats.host_time = t.elapsed();
        match rc {
            Some(0) | None => Ok(()),
            Some(c) => Err(TileError::TileFailed(c)),
        }
    }

    /// Deliver events. Returns whether the tile asked for a redraw.
    pub fn event(&mut self, events: &[Event]) -> Result<bool, TileError> {
        for e in events {
            if let Event::Resize { w, h, .. } = e {
                self.size = (*w, *h);
            }
        }
        let bytes = EventBatch::new(events.to_vec()).encode().map_err(TileError::BadEvents)?;
        let ptr = self.put(&bytes)?;
        let t = Instant::now();
        let rc = self.call_raw("iyakki_tile_event", &[ptr, bytes.len() as i32], Kind::Event)?.unwrap_or(0);
        self.last_stats.host_time = t.elapsed();
        if rc < 0 {
            return Err(TileError::TileFailed(rc));
        }
        Ok(rc > 0)
    }

    /// A tap: pointer down and up at a point, then `Event::Hit` for the topmost hit region of the
    /// last frame there, if any. Returns the hit id.
    pub fn click(&mut self, x: i32, y: i32) -> Result<Option<u32>, TileError> {
        let hit = self.last.as_ref().and_then(|l| l.hit_test(x, y));
        let mut ev = vec![Event::PointerDown { x, y, button: 0 }, Event::PointerUp { x, y, button: 0 }];
        if let Some(id) = hit {
            ev.push(Event::Hit { id });
        }
        self.event(&ev)?;
        Ok(hit)
    }

    /// Ask for the picture. The result is decoded, balanced and checked against the surface size.
    pub fn frame(&mut self) -> Result<DisplayList, TileError> {
        let t = Instant::now();
        let ptr = self.call_raw("iyakki_tile_frame", &[], Kind::Frame)?.unwrap_or(0);
        let fuel = self.last_stats.fuel_used;
        let head = self.session.read_memory(ptr as u32, 4).map_err(|_| TileError::BadPointer)?;
        let len = u32::from_le_bytes([head[0], head[1], head[2], head[3]]) as usize;
        if len > limits::MAX_LIST_BYTES {
            return Err(TileError::BadDisplayList(DecodeError::TooLarge));
        }
        let body = self.session.read_memory((ptr as u32).wrapping_add(4), len).map_err(|_| TileError::BadPointer)?;
        let list = DisplayList::decode(&body).map_err(TileError::BadDisplayList)?;
        list.check_bounds(self.size.0, self.size.1).map_err(|op| TileError::OutOfBounds { op })?;
        self.last_stats = FrameStats { fuel_used: fuel, host_time: t.elapsed(), list_bytes: len };
        self.last = Some(list.clone());
        Ok(list)
    }

    /// The last accepted frame, if any.
    pub fn last_frame(&self) -> Option<&DisplayList> {
        self.last.as_ref()
    }
}

#[cfg(test)]
impl TileRunner {
    pub(crate) fn session_read_bytes(&mut self, ptr: u32, n: usize) -> Vec<u8> {
        self.session.read_memory(ptr, n).unwrap()
    }
    pub(crate) fn session_read_i32(&mut self, ptr: u32) -> i32 {
        let b = self.session_read_bytes(ptr, 4);
        i32::from_le_bytes([b[0], b[1], b[2], b[3]])
    }
}
