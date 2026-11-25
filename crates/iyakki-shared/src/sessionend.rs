//! What ends when a person's session ends (D-SESSION-END, docs/development/ALPHA-8-DECISIONS.md).
//!
//! One registry of end-of-session hooks. Every owner of a long-lived per-person resource (a player, a reader, a capture, a session
//! thread) registers one named hook; `Shell::sign_out` calls [`SessionEnd::end`] once and audits what stopped. A new resource is a
//! new `register` call in `crates/iyakki-agent/src/sessionend_hooks.rs` (or beside the owner in the shell), never a new line in
//! `sign_out`. The audit `docs/development/SESSION-END-AUDIT.md` lists every resource and the rule it falls under.
//!
//! Rules by reason ([`Why`]): a sign-out runs every hook; a change of the sound owner runs the `Sound` hooks of the previous person;
//! a lock runs the `Capture` hooks (playback goes on, as on every desktop OS); a suspend (sleep, lid closed) runs the `Capture` and the
//! `Sound` hooks before the machine sleeps (terminals, forwards and jobs go on: they are not capture and not sound).

use std::sync::{Arc, Mutex, OnceLock, Weak};

/// What a hook ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Something that makes sound (radio, media player, readers, a client's sound route): ends on sign-out and when the sound changes owner.
    Sound,
    /// Microphone, camera, screen capture or sharing, wake listening: ends on sign-out and on lock.
    Capture,
    /// A session process (terminal, forward, sandbox window, job): ends on sign-out only.
    Process,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Sound => "sound",
            Kind::Capture => "capture",
            Kind::Process => "process",
        }
    }
}

/// Why the hooks run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    SignOut,
    /// Another person became the owner of the sound; the hooks end the previous person's sound.
    SoundSwitch,
    Lock,
    /// The machine is about to sleep: capture and sound stop first.
    Suspend,
}

impl Why {
    pub fn name(self) -> &'static str {
        match self {
            Why::SignOut => "sign-out",
            Why::SoundSwitch => "sound-switch",
            Why::Lock => "lock",
            Why::Suspend => "suspend",
        }
    }

    fn runs(self, k: Kind) -> bool {
        match self {
            Why::SignOut => true,
            Why::SoundSwitch => k == Kind::Sound,
            Why::Lock => k == Kind::Capture,
            Why::Suspend => k != Kind::Process,
        }
    }
}

/// What a hook is told.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub user: String,
    pub why: Why,
    /// The person owned the machine's sound (the one signed in last): a machine-wide source with no recorded owner is theirs.
    pub sound_owner: bool,
}

/// A hook returns how many things it stopped (a plain hook: 0 or 1).
pub type Hook = Arc<dyn Fn(&Ctx) -> usize + Send + Sync>;

struct Entry {
    name: String,
    kind: Kind,
    hook: Hook,
    /// The audit line carries the count (`session.end terminals 2`): for hooks that end sessions of a person, which can be several.
    counted: bool,
}

/// What `end` did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Names of the hooks that stopped something, in registration order.
    pub stopped: Vec<String>,
    /// How many things each of those hooks stopped (same order as `stopped`).
    pub counts: Vec<usize>,
    /// How many hooks ran.
    pub ran: usize,
}

impl Report {
    pub fn count(&self) -> usize {
        self.stopped.len()
    }
}

#[derive(Default)]
pub struct SessionEnd {
    hooks: Mutex<Vec<Entry>>,
}

impl SessionEnd {
    pub fn new() -> SessionEnd {
        SessionEnd::default()
    }

    /// Register a hook by name; a second registration of the same name replaces the first (a restarted service registers again).
    pub fn register(&self, name: &str, kind: Kind, hook: impl Fn(&Ctx) -> bool + Send + Sync + 'static) {
        self.put(Entry { name: name.to_string(), kind, hook: Arc::new(move |c| usize::from(hook(c))), counted: false });
    }

    /// Register a hook that says how many things it stopped (sessions, forwards, jobs); the audit line carries the number.
    pub fn register_counted(&self, name: &str, kind: Kind, hook: impl Fn(&Ctx) -> usize + Send + Sync + 'static) {
        self.put(Entry { name: name.to_string(), kind, hook: Arc::new(hook), counted: true });
    }

    fn put(&self, e: Entry) {
        let mut l = self.hooks.lock().unwrap_or_else(|e| e.into_inner());
        match l.iter_mut().find(|x| x.name == e.name) {
            Some(x) => *x = e,
            None => l.push(e),
        }
    }

    /// Take a hook out (a test's probe).
    pub fn unregister(&self, name: &str) {
        self.hooks.lock().unwrap_or_else(|e| e.into_inner()).retain(|e| e.name != name);
    }

    /// Every registered hook: name and kind, in registration order.
    pub fn registered(&self) -> Vec<(String, Kind)> {
        self.hooks.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|e| (e.name.clone(), e.kind)).collect()
    }

    /// Run the hooks the reason asks for. `audit(action, detail)` gets `session.end <name>` for each stop and one `session.end.count`.
    /// The hooks run outside the registry's lock (a hook may register or read the list).
    pub fn end(&self, ctx: &Ctx, audit: &dyn Fn(&str, &str)) -> Report {
        SessionEnd::end_many(&[self], ctx, audit)
    }

    /// [`end`](Self::end) over several registries in one pass with one count: the process-wide one and a shell's own (resources that
    /// only that shell can stop).
    pub fn end_many(regs: &[&SessionEnd], ctx: &Ctx, audit: &dyn Fn(&str, &str)) -> Report {
        let todo: Vec<(String, Hook, bool)> = regs
            .iter()
            .flat_map(|r| r.hooks.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|e| ctx.why.runs(e.kind)).map(|e| (e.name.clone(), e.hook.clone(), e.counted)).collect::<Vec<_>>())
            .collect();
        let mut rep = Report { ran: todo.len(), ..Report::default() };
        for (name, hook, counted) in todo {
            let n = hook(ctx);
            if n > 0 {
                audit("session.end", &if counted { format!("{name} {n}") } else { name.clone() });
                rep.stopped.push(name);
                rep.counts.push(n);
            }
        }
        audit("session.end.count", &format!("{} {} of {}", ctx.why.name(), rep.count(), rep.ran));
        rep
    }

    /// The readable list the chat tool `session_end_report` answers with.
    pub fn describe(&self) -> String {
        let l = self.registered();
        if l.is_empty() {
            return "No end-of-session hooks are registered.".into();
        }
        let mut s = format!("{} things end with the session:", l.len());
        for (n, k) in l {
            s.push_str(&format!("\n- {n} ({})", k.name()));
        }
        s
    }
}

/// The registry of this process (the product registers into it at start; tests use their own `SessionEnd::new()`).
pub fn global() -> &'static SessionEnd {
    static G: OnceLock<SessionEnd> = OnceLock::new();
    G.get_or_init(SessionEnd::new)
}

/// One running thing that a session end can cut: the owner keeps the [`StopGuard`] while it runs and looks at `stopped()` in its loop; the
/// optional `on_stop` closure cuts it at once from the hook's thread (kills a process, closes a device) so nothing waits for the loop.
pub struct StopCell {
    stopped: std::sync::atomic::AtomicBool,
    on_stop: Box<dyn Fn() + Send + Sync>,
}

/// The owner's handle on its [`StopCell`]; dropping it takes the thing out of the set.
pub struct StopGuard(Arc<StopCell>);

impl StopGuard {
    /// The session ended: the owner stops what it runs.
    pub fn stopped(&self) -> bool {
        self.0.stopped.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// The flag itself, for a thread that checks it beside its own stop flag.
    pub fn cell(&self) -> Arc<StopCell> {
        self.0.clone()
    }
}

impl StopCell {
    pub fn stopped(&self) -> bool {
        self.stopped.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Every running thing of one kind that a session end can cut (the players, the recorders, a speaker thread): held weakly, so a thing that
/// ended by itself leaves the set by itself.
pub struct StopSet {
    items: Mutex<Vec<Weak<StopCell>>>,
}

impl Default for StopSet {
    fn default() -> Self {
        StopSet::new()
    }
}

impl StopSet {
    pub const fn new() -> StopSet {
        StopSet { items: Mutex::new(Vec::new()) }
    }

    /// Something starts running. `on_stop` runs once, from the thread that ends the session, when it is cut.
    pub fn enter(&self, on_stop: impl Fn() + Send + Sync + 'static) -> StopGuard {
        let cell = Arc::new(StopCell { stopped: std::sync::atomic::AtomicBool::new(false), on_stop: Box::new(on_stop) });
        let mut l = self.items.lock().unwrap_or_else(|e| e.into_inner());
        l.retain(|w| w.strong_count() > 0);
        l.push(Arc::downgrade(&cell));
        StopGuard(cell)
    }

    /// How many are running now.
    pub fn live(&self) -> usize {
        self.items.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|w| w.strong_count() > 0).count()
    }

    /// Cut everything that runs; returns how many were cut. `on_stop` runs outside the set's lock.
    pub fn stop_all(&self) -> usize {
        let live: Vec<Arc<StopCell>> = self.items.lock().unwrap_or_else(|e| e.into_inner()).iter().filter_map(Weak::upgrade).collect();
        let mut n = 0;
        for c in live {
            if !c.stopped.swap(true, std::sync::atomic::Ordering::SeqCst) {
                (c.on_stop)();
                n += 1;
            }
        }
        n
    }
}

/// A set of live things an owner module can stop all at once (readers made by apps, for example): holds them weakly.
pub struct LiveSet<T> {
    items: Mutex<Vec<Weak<T>>>,
}

impl<T> Default for LiveSet<T> {
    fn default() -> Self {
        LiveSet { items: Mutex::new(Vec::new()) }
    }
}

impl<T> LiveSet<T> {
    pub const fn new() -> LiveSet<T> {
        LiveSet { items: Mutex::new(Vec::new()) }
    }

    pub fn add(&self, item: &Arc<T>) {
        let mut l = self.items.lock().unwrap_or_else(|e| e.into_inner());
        l.retain(|w| w.strong_count() > 0);
        l.push(Arc::downgrade(item));
    }

    /// Call `f` on each live item; returns how many returned true.
    pub fn each(&self, mut f: impl FnMut(&Arc<T>) -> bool) -> usize {
        let live: Vec<Arc<T>> = self.items.lock().unwrap_or_else(|e| e.into_inner()).iter().filter_map(Weak::upgrade).collect();
        live.iter().filter(|i| f(i)).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn ctx(why: Why) -> Ctx {
        Ctx { user: "maya".into(), why, sound_owner: true }
    }

    #[test]
    fn a_sign_out_runs_every_hook_and_audits_each_stop_and_the_count() {
        let reg = SessionEnd::new();
        reg.register("radio", Kind::Sound, |_| true);
        reg.register("camera", Kind::Capture, |_| false);
        reg.register("pty", Kind::Process, |_| true);
        let log = Mutex::new(Vec::new());
        let rep = reg.end(&ctx(Why::SignOut), &|a, d| log.lock().unwrap().push(format!("{a} {d}")));
        assert_eq!(rep.stopped, vec!["radio", "pty"]);
        assert_eq!(rep.ran, 3);
        assert_eq!(*log.lock().unwrap(), vec!["session.end radio", "session.end pty", "session.end.count sign-out 2 of 3"]);
    }

    #[test]
    fn a_sound_switch_runs_only_sound_and_a_lock_only_capture() {
        let reg = SessionEnd::new();
        let n = Arc::new(AtomicUsize::new(0));
        for (name, k) in [("a", Kind::Sound), ("b", Kind::Capture), ("c", Kind::Process)] {
            let n = n.clone();
            reg.register(name, k, move |_| {
                n.fetch_add(1, Ordering::SeqCst);
                true
            });
        }
        assert_eq!(reg.end(&ctx(Why::SoundSwitch), &|_, _| {}).stopped, vec!["a"]);
        assert_eq!(reg.end(&ctx(Why::Lock), &|_, _| {}).stopped, vec!["b"]);
        assert_eq!(n.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn two_registries_end_in_one_pass_with_one_count() {
        let (a, b) = (SessionEnd::new(), SessionEnd::new());
        a.register("radio", Kind::Sound, |_| true);
        b.register("wake", Kind::Capture, |_| true);
        let log = Mutex::new(Vec::new());
        let rep = SessionEnd::end_many(&[&a, &b], &ctx(Why::SignOut), &|x, d| log.lock().unwrap().push(format!("{x} {d}")));
        assert_eq!(rep.stopped, vec!["radio", "wake"]);
        assert_eq!(log.lock().unwrap().last().unwrap(), "session.end.count sign-out 2 of 2");
    }

    #[test]
    fn registering_a_name_twice_replaces_the_hook() {
        let reg = SessionEnd::new();
        reg.register("radio", Kind::Sound, |_| false);
        reg.register("radio", Kind::Sound, |_| true);
        assert_eq!(reg.registered().len(), 1);
        assert_eq!(reg.end(&ctx(Why::SignOut), &|_, _| {}).count(), 1);
        assert!(reg.describe().contains("- radio (sound)"));
    }

    #[test]
    fn a_counted_hook_audits_how_many_it_stopped() {
        let reg = SessionEnd::new();
        reg.register_counted("terminals", Kind::Process, |_| 3);
        reg.register_counted("forwards", Kind::Process, |_| 0);
        reg.register("radio", Kind::Sound, |_| true);
        let log = Mutex::new(Vec::new());
        let rep = reg.end(&ctx(Why::SignOut), &|a, d| log.lock().unwrap().push(format!("{a} {d}")));
        assert_eq!((rep.stopped.clone(), rep.counts.clone()), (vec!["terminals".to_string(), "radio".to_string()], vec![3, 1]));
        assert_eq!(*log.lock().unwrap(), vec!["session.end terminals 3", "session.end radio", "session.end.count sign-out 2 of 3"]);
    }

    #[test]
    fn a_suspend_runs_capture_and_sound_and_leaves_processes() {
        let reg = SessionEnd::new();
        for (name, k) in [("a", Kind::Sound), ("b", Kind::Capture), ("c", Kind::Process)] {
            reg.register(name, k, |_| true);
        }
        let rep = reg.end(&ctx(Why::Suspend), &|_, _| {});
        assert_eq!(rep.stopped, vec!["a", "b"]);
        assert_eq!(Why::Suspend.name(), "suspend");
    }

    #[test]
    fn a_stop_set_cuts_what_runs_once_and_forgets_what_ended() {
        let set = StopSet::new();
        let cut = Arc::new(AtomicUsize::new(0));
        let c2 = cut.clone();
        let a = set.enter(move || {
            c2.fetch_add(1, Ordering::SeqCst);
        });
        {
            let _gone = set.enter(|| panic!("an ended thing is not cut"));
        }
        assert_eq!(set.live(), 1);
        assert!(!a.stopped());
        assert_eq!(set.stop_all(), 1);
        assert!(a.stopped() && a.cell().stopped());
        assert_eq!(set.stop_all(), 0, "cut once");
        assert_eq!(cut.load(Ordering::SeqCst), 1);
        let b = set.enter(|| {});
        assert!(!b.stopped(), "a new run is not stopped by an old end");
    }

    #[test]
    fn a_live_set_holds_items_weakly() {
        let set = LiveSet::<AtomicUsize>::new();
        let a = Arc::new(AtomicUsize::new(0));
        set.add(&a);
        {
            let b = Arc::new(AtomicUsize::new(0));
            set.add(&b);
        }
        assert_eq!(set.each(|x| {
            x.fetch_add(1, Ordering::SeqCst);
            true
        }), 1);
        assert_eq!(a.load(Ordering::SeqCst), 1);
    }
}
