//! The lead assistant's tool for the cloud tenant's storage use (F-CLOUD-08): `cloud_storage_usage` says how much the person's own folders
//! and the tenant hold and how close that is to the limits of `storage.quota` ([`crate::cloudquota`]). It only reads: it counts files, never
//! creates a folder, and shows names and sizes only (no file content). A person sees the folders of the teams they are in, a tenant
//! administrator also the tenant's total. Without a roles file (no cloud tenant on this machine) it says so.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::cloudaccess::Access;
use crate::cloudquota::{usage, StorageQuota};
use crate::cloudroles::TenantRole;
use crate::cloudstorage::{Scope, StorageLayout};

pub const TOOL: &str = "cloud_storage_usage";
/// The tenant's audit trail of people and access changes (the same lines the Cloud access page of Settings shows at its bottom).
pub const AUDIT_TOOL: &str = "cloud_audit_trail";
/// Most lines one call returns.
pub const AUDIT_MAX: usize = 100;

pub const PROMPT_RULE: &str = "\nCLOUD STORAGE: cloud_storage_usage shows how much space the person's folders in the cloud tenant use and the limits (read only, free). cloud_audit_trail lists the newest lines of the tenant's audit trail (who changed whose role, who was added or removed; names and actions only, tenant administrators only, read only, free).";

pub fn is_cloud_storage_tool(n: &str) -> bool {
    n == TOOL || n == AUDIT_TOOL
}

/// `unix seconds` as `YYYY-MM-DD HH:MM` (UTC).
fn utc_text(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // civil from days (proleptic Gregorian, days since 1970-01-01)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", rem / 3600, rem % 3600 / 60)
}

/// The newest `count` (1 to [`AUDIT_MAX`]) lines of the tenant's audit trail, newest first. Only a tenant administrator or owner may read
/// it; it reads the file and changes nothing. Without a cloud tenant it says so.
pub fn audit_report(data_dir: &std::path::Path, user: &str, count: usize) -> Result<String, String> {
    let access = Access::new(data_dir);
    let Some(roles) = access.roles()? else { return Ok("There is no cloud tenant on this computer, so there is no audit trail.".into()) };
    match roles.tenant_role(user) {
        Some(r) if r >= TenantRole::Admin => {}
        _ => return Err("no access".into()),
    }
    let count = count.clamp(1, AUDIT_MAX);
    let text = match std::fs::read_to_string(crate::cloudroles::audit_path(data_dir)) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("the audit trail cannot be read: {e}")),
    };
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return Ok("The audit trail is empty.".into());
    }
    let shown: Vec<String> = lines
        .iter()
        .rev()
        .take(count)
        .map(|l| match l.split_once('\t') {
            Some((t, what)) => match t.trim().parse::<u64>() {
                Ok(secs) => format!("{} UTC  {what}", utc_text(secs)),
                Err(_) => (*l).to_string(),
            },
            None => (*l).to_string(),
        })
        .collect();
    Ok(format!("Audit trail of the cloud tenant, newest first ({} of {} lines):\n{}", shown.len(), lines.len(), shown.join("\n")))
}

fn size_text(b: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    if b >= 1024 * MIB { format!("{:.1} GiB", b as f64 / (1024.0 * MIB as f64)) } else if b >= MIB { format!("{} MiB", b / MIB) } else { format!("{} KiB", b.div_ceil(1024)) }
}

fn percent(used: u64, limit: u64) -> u64 {
    if limit == 0 { 100 } else { (used.saturating_mul(100) / limit).min(999) }
}

fn line(label: &str, used: u64, limit: u64) -> String {
    let p = percent(used, limit);
    let warn = if used > limit { " (OVER the limit)" } else if p >= 90 { " (nearly full)" } else { "" };
    format!("• {label}: {} of {} ({p}%){warn}", size_text(used), size_text(limit))
}

/// The report for `user`. Pure over the data directory (reads only).
pub fn report(data_dir: &std::path::Path, user: &str) -> Result<String, String> {
    let access = Access::new(data_dir);
    let Some(roles) = access.roles()? else { return Ok("There is no cloud tenant on this computer, so there is no tenant storage to count.".into()) };
    let Some(role) = roles.tenant_role(user) else { return Err("no access".into()) };
    let q = StorageQuota::load(data_dir, None);
    let root = access.files_root();
    let layout = StorageLayout::default();
    let mut out = vec![format!("Storage of the cloud tenant (limits: {} per folder, {} for the tenant).", size_text(q.place_bytes), size_text(q.tenant_bytes))];
    let mut count = |label: String, scope: Scope| -> Result<(), String> {
        let place = layout.locate(user, &scope)?;
        let dir = layout.mount(&root, &place)?.join(&place.subpath);
        out.push(line(&label, usage(&dir)?, q.place_bytes));
        Ok(())
    };
    count("your personal folder".into(), Scope::Personal)?;
    for (ns, mine) in roles.visible_namespaces(user) {
        if mine.is_none() {
            continue;
        }
        count(format!("team {ns}, your work"), Scope::Person { namespace: ns.clone() })?;
        count(format!("team {ns}, shared"), Scope::Shared { namespace: ns })?;
    }
    if role >= TenantRole::Admin {
        out.push(line("the whole tenant", usage(&root)?, q.tenant_bytes));
    }
    Ok(out.join("\n"))
}

pub struct CloudStorageTools {
    inner: Arc<dyn Tools>,
    data_dir: PathBuf,
    user: String,
}

impl CloudStorageTools {
    pub fn new(inner: Arc<dyn Tools>, data_dir: PathBuf, user: &str) -> CloudStorageTools {
        CloudStorageTools { inner, data_dir, user: user.to_string() }
    }
}

impl Tools for CloudStorageTools {

    fn inner_tools(&self) -> Option<&dyn Tools> {

        Some(&*self.inner)

    }
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| d.spec.name != TOOL && d.spec.name != AUDIT_TOOL);
        v.push(ToolDef { spec: ToolSpec { name: AUDIT_TOOL.into(), description: "List the newest lines of the cloud tenant's audit trail (role changes, people added or removed, namespaces made or deleted), newest first. Tenant administrators only. Read only.".into(), schema: json!({"type": "object", "properties": {"count": {"type": "integer", "minimum": 1, "maximum": 100, "description": "How many lines, default 20"}}}), }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef { spec: ToolSpec { name: TOOL.into(), description: "Show how much space the person's folders in the cloud tenant use, per folder, against the limits (and the tenant's total for an administrator). Read only.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        if name == TOOL {
            report(&self.data_dir, &self.user)
        } else if name == AUDIT_TOOL {
            let n = args.get("count").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
            audit_report(&self.data_dir, &self.user, n)
        } else {
            self.inner.call(name, args)
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        if name == TOOL {
            "Show the cloud tenant's storage use".into()
        } else if name == AUDIT_TOOL {
            "Show the cloud tenant's audit trail".into()
        } else {
            self.inner.describe(name, args)
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if name == TOOL || name == AUDIT_TOOL { None } else { self.inner.preview(name, args) }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if name == TOOL || name == AUDIT_TOOL { None } else { self.inner.refusal(name, args) }
    }

    fn pre_approved(&self, name: &str) -> bool {
        self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        self.inner.grant_blocker(name, args)
    }

    fn grant_scope(&self, name: &str, args: &Value) -> String {
        self.inner.grant_scope(name, args)
    }

    fn unresolved_work(&self) -> Vec<String> {
        self.inner.unresolved_work()
    }

    fn begin_step(&self) {
        self.inner.begin_step()
    }

    fn should_end_run(&self) -> Option<String> {
        self.inner.should_end_run()
    }

    fn take_diagnosis(&self) -> Option<(String, String)> {
        self.inner.take_diagnosis()
    }

    fn note_external(&self, name: &str, args: &Value, ok: bool) {
        self.inner.note_external(name, args, ok)
    }

    fn take_notes(&self) -> Vec<String> {
        self.inner.take_notes()
    }

    fn jobs_running(&self) -> Vec<String> {
        self.inner.jobs_running()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-cst-{tag}-{}", crate::cloudkeys::hex(&crate::cloudkeys::random::<6>().unwrap())));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn put(dir: &std::path::Path, rel: &str, bytes: usize) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, vec![b'x'; bytes]).unwrap();
    }

    #[test]
    fn without_a_cloud_tenant_it_says_so_and_a_stranger_gets_no_access() {
        let d = data("a");
        assert!(report(&d, "alice").unwrap().contains("no cloud tenant"));
        Access::new(&d).bootstrap("alice").unwrap();
        assert_eq!(report(&d, "mallory").unwrap_err(), "no access");
    }

    #[test]
    fn a_member_sees_their_own_folders_and_an_owner_also_the_tenant_total_and_nothing_is_created() {
        let d = data("b");
        let a = Access::new(&d);
        a.bootstrap("alice").unwrap();
        a.change(|r| r.create_namespace("alice", "ops")).unwrap();
        a.change(|r| r.add_tenant_member("alice", "bob", TenantRole::Member)).unwrap();
        a.change(|r| r.add_namespace_member("alice", "ops", "bob", crate::cloudroles::NsRole::Editor)).unwrap();
        let root = a.files_root();
        put(&root, "ns/ops/people/alice/a.bin", 3 * 1024 * 1024);
        put(&root, "ns/ops/people/bob/b.bin", 1024 * 1024);
        put(&root, "ns/ops/shared/s.bin", 2 * 1024 * 1024);
        let before = std::fs::read_dir(&root).unwrap().count();
        let alice = report(&d, "alice").unwrap();
        assert!(alice.contains("team ops, your work: 3 MiB of 10.0 GiB"), "{alice}");
        assert!(alice.contains("team ops, shared: 2 MiB"), "{alice}");
        assert!(alice.contains("the whole tenant: 6 MiB of 100.0 GiB"), "{alice}");
        let bob = report(&d, "bob").unwrap();
        assert!(bob.contains("team ops, your work: 1 MiB") && !bob.contains("whole tenant"), "{bob}");
        assert!(!bob.contains("a.bin") && !bob.contains("alice"));
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), before);
        assert!(!root.join("personal").exists());
    }

    #[test]
    fn the_tool_is_free_forwards_everything_else_and_marks_a_full_folder() {
        let d = data("c");
        let t = CloudStorageTools::new(Arc::new(crate::assign::tests_support::NoTools), d, "alice");
        let defs = t.defs();
        assert!(matches!(defs.iter().find(|x| x.spec.name == TOOL).map(|x| x.risk), Some(Risk::Safe)));
        assert!(is_cloud_storage_tool(TOOL) && !is_cloud_storage_tool("read_file"));
        assert!(t.refusal(TOOL, &json!({})).is_none() && t.preview(TOOL, &json!({})).is_none());
        assert!(t.call(TOOL, &json!({})).unwrap().contains("no cloud tenant"));
        assert!(line("x", 11, 10).contains("OVER") && line("x", 95, 100).contains("nearly full") && !line("x", 5, 100).contains("full"));
        assert_eq!(percent(1, 0), 100);
    }

    #[test]
    fn the_audit_trail_tool_reads_the_newest_lines_for_administrators_only_and_changes_nothing() {
        let d = data("audit");
        assert!(audit_report(&d, "alice", 5).unwrap().contains("no cloud tenant"));
        let a = Access::new(&d);
        a.bootstrap("alice").unwrap();
        a.change(|r| r.add_tenant_member("alice", "bob", TenantRole::Member)).unwrap();
        let f = crate::cloudroles::audit_path(&d);
        assert!(audit_report(&d, "alice", 5).unwrap().contains("2 of 2 lines"), "the two changes above wrote their own lines");
        std::fs::remove_file(&f).unwrap();
        assert_eq!(audit_report(&d, "alice", 5).unwrap(), "The audit trail is empty.");
        for (i, t) in [0u64, 1_700_000_000, 1_794_485_955].iter().enumerate() {
            crate::cloudroles::append_audit(&f, *t, &crate::cloudroles::Change(format!("tenant: alice did change {i}"))).unwrap();
        }
        let before = std::fs::read_to_string(&f).unwrap();
        let all = audit_report(&d, "alice", 20).unwrap();
        assert!(all.contains("3 of 3 lines"), "{all}");
        let pos = |s: &str| all.find(s).unwrap();
        assert!(pos("change 2") < pos("change 1") && pos("change 1") < pos("change 0"), "newest first: {all}");
        assert!(all.contains("1970-01-01 00:00 UTC") && all.contains("2023-11-14 22:13 UTC"), "{all}");
        let one = audit_report(&d, "alice", 1).unwrap();
        assert!(one.contains("1 of 3 lines") && one.contains("change 2") && !one.contains("change 1"));
        assert_eq!(audit_report(&d, "alice", 0).unwrap().matches("did change").count(), 1, "zero is read as one");
        assert_eq!(audit_report(&d, "bob", 5).unwrap_err(), "no access", "a plain member");
        assert_eq!(audit_report(&d, "mallory", 5).unwrap_err(), "no access", "a stranger");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), before, "reading changes nothing");
        let t = CloudStorageTools::new(Arc::new(crate::assign::tests_support::NoTools), d, "alice");
        assert!(matches!(t.defs().iter().find(|x| x.spec.name == AUDIT_TOOL).map(|x| x.risk), Some(Risk::Safe)));
        assert!(is_cloud_storage_tool(AUDIT_TOOL) && t.refusal(AUDIT_TOOL, &json!({})).is_none());
        assert!(t.call(AUDIT_TOOL, &json!({"count": 2})).unwrap().contains("2 of 3 lines"));
    }
}
