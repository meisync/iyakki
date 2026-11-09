//! DISKS, encrypted volumes: the engine side of unlocking. A LUKS or BitLocker partition is opened by the helper
//! (`disks_helper.rs`) into `/dev/mapper/iyakki-<partition>`, the file system inside is read from that device, and it is mounted
//! READ-ONLY. Writing is a second, separate step ([`Disks::allow_writing`]). Locking unmounts and closes it again.
//!
//! Rules that hold here, whatever the screen or the assistant does: the key reaches the helper only through its standard input; it
//! is never in an audit line, a message or an error; a wrong key costs a growing wait; nothing is written to the drive (a
//! BitLocker volume is never made writable); the system drive and other people's drives are refused like any mount.

use std::sync::OnceLock;
use std::time::Instant;

use crate::disks::{detect, AuditFn, Actor, Disks, Fs, MountOpts, Mounted, Part, Snapshot, PROBE_BYTES};
use crate::disks_crypt::{mapper_path, CryptKind, Secret};
use crate::disks_helper::{CryptRunner, OpenError, OpenReq};

/// Seconds on a clock that only goes forward (for the wrong-key wait).
fn now_secs() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs()
}

impl Disks {
    /// For tests: make the wrong-key wait use a clock that stands still.
    pub fn hold_clock(&self, secs: u64) {
        self.held_clock.store(secs, std::sync::atomic::Ordering::SeqCst);
    }

    fn now(&self) -> u64 {
        match self.held_clock.load(std::sync::atomic::Ordering::SeqCst) {
            u64::MAX => now_secs(),
            t => t,
        }
    }

    fn limiter_guard(&self) -> std::sync::MutexGuard<'_, crate::disks_crypt::Limiter> {
        self.limiter.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The file system inside an unlocked container, as a partition of its own (`None` when the opened device cannot be read).
    pub(crate) fn inner_part(&self, p: &Part) -> Option<Part> {
        let dev = mapper_path(&p.name);
        let b = self.env.dev_read(&dev, 0, PROBE_BYTES).ok()?;
        let probe = detect(&b);
        let mut inner = p.clone();
        inner.path = dev;
        inner.fs = probe.fs;
        inner.label = probe.label;
        inner.uuid = probe.uuid;
        inner.crypt = None;
        inner.unlocked = false;
        inner.mountpoints.clear();
        inner.space = None;
        inner.mounted_ro = false;
        Some(inner)
    }

    /// Whether a partition can be unlocked now, and as what. Pure: nothing is changed. The wrong-key wait is checked by [`unlock`](Self::unlock).
    pub fn plan_unlock(&self, snap: &Snapshot, who: &Actor, part: &str, runner: &dyn CryptRunner) -> Result<CryptKind, String> {
        let (d, p) = snap.part(part).ok_or_else(|| format!("{part}: there is no such partition (the drive may have been unplugged)."))?;
        crate::disks::may_use(who, d)?;
        if p.system {
            return Err(format!("{} is part of the running system and stays as it is.", p.name));
        }
        let Some(c) = &p.crypt else { return Err(format!("{} is not an encrypted LUKS or BitLocker volume, so there is nothing to unlock.", p.name)) };
        if let Some(why) = &c.problem {
            return Err(format!("{} cannot be unlocked: {why}.", p.name));
        }
        if p.unlocked {
            return Err(format!("{} is already unlocked{}.", p.name, if p.mounted() { format!(" and open at {}", p.mountpoints[0]) } else { " (choose Mount to open it)".into() }));
        }
        runner.available(c.kind)?;
        Ok(c.kind)
    }

    /// Unlock a partition with the key a person typed and open it read-only. The secret is only handed to the helper.
    pub fn unlock(&self, who: &Actor, part: &str, secret: &Secret, runner: &dyn CryptRunner, audit: AuditFn) -> Result<Mounted, String> {
        let snap = self.scan();
        let kind = self.plan_unlock(&snap, who, part, runner).inspect_err(|e| audit("disks.unlock.refused", &format!("part={part} why={e}")))?;
        let (d, p) = snap.part(part).ok_or("The drive is gone.")?;
        if let Err(wait) = self.limiter_guard().check(&p.name, self.now()) {
            let m = format!("Too many wrong tries. Wait {wait} seconds, then try again. (This protects the drive from guessing.)");
            audit("disks.unlock.refused", &format!("{} part={} why=rate limit", d.audit_id(), p.name));
            return Err(m);
        }
        if secret.is_empty() {
            return Err(format!("Type the {} first.", kind.secret_word()));
        }
        let ident = format!("{} part={} kind={}", d.audit_id(), p.name, kind.name());
        // BitLocker is only ever opened for reading here; a LUKS device can be written later, but it is mounted read-only first
        let req = OpenReq { kind, dev: &p.path, part: &p.name, read_only: kind == CryptKind::BitLocker };
        match runner.open(&req, secret) {
            Ok(()) => {}
            Err(OpenError::WrongKey) => {
                let n = {
                    let mut l = self.limiter_guard();
                    l.failed(&p.name, self.now());
                    l.fails(&p.name)
                };
                audit("disks.unlock.failed", &format!("{ident} why=wrong key ({n} in a row)"));
                let wait = crate::disks_crypt::delay_after(n);
                return Err(format!("That {} did not unlock {}. Check it and try again (wait {wait} seconds first). Nothing on the drive was changed.", kind.secret_word().split(" or ").next().unwrap_or("key"), p.title()));
            }
            Err(OpenError::Failed(e)) => {
                audit("disks.unlock.failed", &format!("{ident} error={e}"));
                return Err(e);
            }
        }
        self.limiter_guard().ok(&p.name);
        // from here on a failure locks the volume again, so nothing is left half open
        let undo = |why: String| -> String {
            let _ = runner.close(kind, &p.name);
            self.forget();
            audit("disks.unlock.failed", &format!("{ident} error={why}"));
            why
        };
        let Some(inner) = self.inner_part(p) else {
            return Err(undo("The volume unlocked, but its contents could not be read. It is locked again.".into()));
        };
        if matches!(inner.fs, Fs::Empty | Fs::Unknown) {
            return Err(undo(format!("The volume unlocked, but what is inside is {} (nothing IYAKKI can open). It is locked again.", if inner.fs == Fs::Empty { "empty" } else { "in a format it does not know" })));
        }
        let plan = match self.plan_for(&snap, who, d, &inner, &MountOpts { read_only: true, allow_exec: false }) {
            Ok(pl) => pl,
            Err(e) => return Err(undo(format!("{e} The volume is locked again."))),
        };
        match self.run_mount(who, d, &inner, plan, audit) {
            Ok(mut m) => {
                self.forget();
                audit("disks.unlock", &format!("{ident} inner={} at={}", inner.fs.name(), m.target));
                m.read_only = true;
                m.notes.insert(0, "Unlocked and opened read-only. Nothing can be changed until you choose Allow writing.".into());
                Ok(m)
            }
            Err(e) => Err(undo(format!("{e} The volume is locked again."))),
        }
    }

    /// The second step: open an unlocked, read-only volume for writing. Refused for BitLocker (the helper's write support is experimental).
    pub fn allow_writing(&self, who: &Actor, part: &str, audit: AuditFn) -> Result<Mounted, String> {
        let snap = self.scan();
        let (d, p) = snap.part(part).ok_or_else(|| format!("{part}: there is no such partition."))?;
        crate::disks::may_use(who, d).inspect_err(|e| audit("disks.write.refused", &format!("part={part} why={e}")))?;
        let fail = |why: String| -> Result<Mounted, String> {
            audit("disks.write.refused", &format!("{} part={} why={why}", d.audit_id(), p.name));
            Err(why)
        };
        match &p.crypt {
            Some(c) if p.unlocked && p.mounted() => {
                if c.kind == CryptKind::BitLocker {
                    return fail("A BitLocker volume stays read-only here: the helper's write support is experimental and could damage the volume. Copy what you need from it, or write to it in Windows.".into());
                }
            }
            Some(_) if p.unlocked => return fail(format!("{} is unlocked but not open. Choose Mount first.", p.name)),
            _ => return fail(format!("{} is not an unlocked volume.", p.name)),
        }
        if d.read_only {
            return fail("The drive is write-protected (a switch on it, or the device itself), so it cannot be opened for writing.".into());
        }
        if !p.mounted_ro {
            return fail(format!("{} is already open for writing.", p.name));
        }
        let Some(inner) = self.inner_part(p) else { return fail("The unlocked volume cannot be read any more.".into()) };
        Self::own_mounts_only(who, p).or_else(|e| fail(e).map(|_| ()))?;
        self.unmount_part(d, p, audit)?;
        let reopen = |ro: bool| -> Result<Mounted, String> {
            let plan = self.plan_for(&snap, who, d, &inner, &MountOpts { read_only: ro, allow_exec: false })?;
            self.run_mount(who, d, &inner, plan, audit)
        };
        match reopen(false) {
            Ok(m) => {
                self.forget();
                audit("disks.write.allowed", &format!("{} part={} at={}", d.audit_id(), p.name, m.target));
                Ok(m)
            }
            Err(e) => {
                let back = reopen(true).map(|m| format!(" It is open read-only again at {}.", m.target)).unwrap_or_default();
                self.forget();
                Err(format!("{e}{back}"))
            }
        }
    }

    /// Lock again: unmount whatever is open from the volume, then close the opened device. Never forced.
    pub fn lock(&self, who: &Actor, part: &str, runner: &dyn CryptRunner, audit: AuditFn) -> Result<String, String> {
        let snap = self.scan();
        let (d, p) = snap.part(part).ok_or_else(|| format!("{part}: there is no such partition."))?;
        crate::disks::may_use(who, d).inspect_err(|e| audit("disks.lock.refused", &format!("part={part} why={e}")))?;
        let Some(c) = p.crypt.as_ref().filter(|_| p.unlocked) else { return Err(format!("{} is not unlocked.", p.name)) };
        Self::own_mounts_only(who, p).inspect_err(|e| audit("disks.lock.refused", &format!("{} part={part} why={e}", d.audit_id())))?;
        if p.mounted() {
            self.unmount_part(d, p, audit)?;
        }
        if let Err(e) = runner.close(c.kind, &p.name) {
            self.forget();
            audit("disks.lock.failed", &format!("{} part={} error={e}", d.audit_id(), p.name));
            return Err(format!("{e} The folder was closed, but the volume is still unlocked."));
        }
        self.forget();
        audit("disks.lock", &format!("{} part={} kind={}", d.audit_id(), p.name, c.kind.name()));
        Ok(format!("{} is locked again. Nothing was lost; it needs the {} to open.", p.title(), c.kind.secret_word().split(" or ").next().unwrap_or("key")))
    }
}
