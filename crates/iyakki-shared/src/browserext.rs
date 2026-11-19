// The browser extension host: the small registry that builds the extension snapshot for the full web engine
// (docs/development/BROWSER-EXTENSIONS-PLAN.md). Not an extension system yet: built-in extensions register here, each declares the
// capabilities it wants (names from `iyakki_core_api::appcaps`), and at snapshot-build time the broker decides which it holds. A hook
// whose capability is not granted is left out, so an extension without its capability gets no hook. The hook kinds and the wire layout
// are in `iyakki_webview::ext`; the only kind with code is `request.headers`, and the first (so far only) built-in extension is the
// custom request headers (`crate::browserheaders`, F-BROW-79).
//
// Secrets stay in the vault: an extension builds its hook payload from what the shell already holds in memory (the header values read
// from the vault when the person signed in), the snapshot goes only over the helper socket, and nothing here logs a payload.
// `Debug` for every type here shows names and counts only.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

use iyakki_core_api::appcaps::MemoryBook;
use iyakki_core_api::broker::{Broker, Call, Caller, CallerKind, NoCard, NullSink};
use iyakki_webview::ext::{self, hook, Entry, ExtBlob, Hook, Snapshot};

/// A built-in browser extension.
pub trait BrowserExtension: Send + Sync {
    /// Its app id (the broker's built-in table is keyed by it).
    fn id(&self) -> &str;
    fn version(&self) -> &str;
    /// The capabilities it asks for.
    fn capabilities(&self) -> Vec<String>;
    /// Its hooks right now (kind and payload). Called each time a snapshot is built.
    fn hooks(&self) -> Vec<Hook>;
}

/// Who decides whether an extension holds a capability.
pub trait CapGate: Send + Sync {
    fn allows(&self, extension: &str, capability: &str) -> bool;
}

/// The broker, asked as a built-in app of the signed-in person.
pub struct BrokerGate {
    pub broker: Broker,
    pub person: String,
    pub device: String,
}

impl BrokerGate {
    /// A gate that holds only what the signed built-in table gives (no store approvals, no cards).
    pub fn builtin(person: &str, device: &str) -> BrokerGate {
        BrokerGate { broker: Broker::new(Arc::new(MemoryBook::default()), Arc::new(NoCard), Arc::new(NullSink), Arc::new(|| iyakki_schema::now_secs() as i64)), person: person.into(), device: device.into() }
    }
}

impl CapGate for BrokerGate {
    fn allows(&self, extension: &str, capability: &str) -> bool {
        let caller = Caller { app: extension.into(), kind: CallerKind::Builtin, person: self.person.clone(), device: self.device.clone() };
        self.broker.decide(&Call { caller: &caller, capability, unit: None, label: "browser extension" }).is_ok()
    }
}

pub struct BrowserExtensionHost {
    exts: RwLock<Vec<Arc<dyn BrowserExtension>>>,
    gate: RwLock<Arc<dyn CapGate>>,
    generation: AtomicU64,
}

impl std::fmt::Debug for BrowserExtensionHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = self.exts.read().map(|e| e.len()).unwrap_or(0);
        write!(f, "BrowserExtensionHost({n} extensions, generation {})", self.generation())
    }
}

impl BrowserExtensionHost {
    pub fn new(gate: Arc<dyn CapGate>) -> BrowserExtensionHost {
        BrowserExtensionHost { exts: RwLock::new(Vec::new()), gate: RwLock::new(gate), generation: AtomicU64::new(1) }
    }

    pub fn register(&self, e: Arc<dyn BrowserExtension>) {
        if let Ok(mut v) = self.exts.write() {
            v.retain(|x| x.id() != e.id());
            v.push(e);
        }
        self.changed();
    }

    pub fn set_gate(&self, g: Arc<dyn CapGate>) {
        if let Ok(mut w) = self.gate.write() {
            *w = g;
        }
        self.changed();
    }

    /// Something an extension serves changed: the full engine needs a new snapshot.
    pub fn changed(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Counts up with every change; the full engine's view sends a new snapshot when it differs from the one it sent.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    /// The snapshot: each extension with the capabilities the broker granted and only the hooks those capabilities allow.
    pub fn snapshot(&self) -> Snapshot {
        let gate = self.gate.read().map(|g| g.clone());
        let Ok(gate) = gate else { return Snapshot::default() };
        let mut entries = Vec::new();
        for e in self.exts.read().map(|v| v.clone()).unwrap_or_default() {
            let granted: Vec<String> = e.capabilities().into_iter().filter(|c| gate.allows(e.id(), c)).collect();
            let hooks: Vec<Hook> = e.hooks().into_iter().filter(|h| hook::capability(h.kind).is_some_and(|need| granted.iter().any(|g| g == need))).collect();
            if !hooks.is_empty() {
                entries.push(Entry { id: e.id().to_string(), version: e.version().to_string(), caps: granted, hooks });
            }
        }
        Snapshot { entries }
    }

    /// The bytes for `ToHelper::Extensions`, with the generation they were built at.
    pub fn blob(&self) -> (u64, ExtBlob) {
        let g = self.generation();
        (g, self.snapshot().encode())
    }
}

/// The header rules as the first built-in extension.
pub struct HeaderRulesExtension;

pub const HEADERS_ID: &str = "browser-headers";

impl BrowserExtension for HeaderRulesExtension {
    fn id(&self) -> &str {
        HEADERS_ID
    }
    fn version(&self) -> &str {
        "1"
    }
    fn capabilities(&self) -> Vec<String> {
        vec!["web.request.headers".into()]
    }
    fn hooks(&self) -> Vec<Hook> {
        let (rules, _skipped) = wire_rules();
        if rules.is_empty() {
            return Vec::new();
        }
        vec![Hook { kind: hook::REQUEST_HEADERS, payload: ext::encode_header_rules(&rules) }]
    }
}

/// Where the active header rules come from: the core registers its store here (`iyakki_agent::browserheaders::activate`); until then there are none.
static WIRE_RULES: RwLock<Option<fn() -> (Vec<ext::HeaderRule>, usize)>> = RwLock::new(None);

/// The core says where the active header rules are read from.
pub fn set_wire_rules(f: fn() -> (Vec<ext::HeaderRule>, usize)) {
    if let Ok(mut w) = WIRE_RULES.write() {
        *w = Some(f);
    }
}

fn wire_rules() -> (Vec<ext::HeaderRule>, usize) {
    WIRE_RULES.read().ok().and_then(|w| *w).map_or((Vec::new(), 0), |f| f())
}

static HOST: OnceLock<BrowserExtensionHost> = OnceLock::new();

/// The host the shell uses: the built-in extensions registered, the broker gate for a built-in app.
pub fn host() -> &'static BrowserExtensionHost {
    HOST.get_or_init(|| {
        let h = BrowserExtensionHost::new(Arc::new(BrokerGate::builtin("", "")));
        // not through register(): that calls changed(), which is fine, but keeps the first generation at 1
        if let Ok(mut v) = h.exts.write() {
            v.push(Arc::new(HeaderRulesExtension));
        }
        h
    })
}
