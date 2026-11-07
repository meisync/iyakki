//! Fakes only: cgroup text, a made-up pod list, a scripted kubectl. No cluster, no network, no hardware.

use super::*;
use crate::sysinfo::SysRoots;
use std::sync::Mutex;

const UID: &str = "7c1e2f3a-aaaa-4bbb-8ccc-0123456789ab";
const CID: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn v2(uid_us: &str, id: &str) -> String {
    format!("0::/kubepods.slice/kubepods-burstable.slice/kubepods-burstable-pod{uid_us}.slice/cri-containerd-{id}.scope\n")
}

fn pods_json() -> String {
    format!(
        r#"{{"items":[
 {{"metadata":{{"uid":"{UID}","namespace":"iyakki-data","name":"clickhouse-0","labels":{{"app.kubernetes.io/name":"clickhouse"}}}},
   "spec":{{"containers":[{{"name":"clickhouse","resources":{{"requests":{{"cpu":"250m","memory":"256Mi"}},"limits":{{"cpu":"1","memory":"1Gi"}}}}}}]}},
   "status":{{"containerStatuses":[{{"name":"clickhouse","containerID":"containerd://{CID}"}}]}}}},
 {{"metadata":{{"uid":"11111111-2222-4333-8444-555555555555","namespace":"iyakki-git","name":"gitea-6d9f8b7c5-x2k4q","ownerReferences":[{{"kind":"ReplicaSet","name":"gitea-6d9f8b7c5"}}]}},
   "spec":{{"containers":[{{"name":"gitea"}}]}},"status":{{}}}}
]}}"#
    )
}

struct Fake {
    calls: Mutex<Vec<String>>,
    pods: Result<String, String>,
    top: String,
    log: String,
}

impl Fake {
    fn up() -> Arc<Fake> {
        Arc::new(Fake { calls: Mutex::new(vec![]), pods: Ok(pods_json()), top: "clickhouse-0   clickhouse   1020m   775Mi\n".into(), log: "start\npassword=hunter2 connecting\n".into() })
    }
    fn down() -> Arc<Fake> {
        Arc::new(Fake { calls: Mutex::new(vec![]), pods: Err("k3s is stopped".into()), top: String::new(), log: String::new() })
    }
}

impl Kubectl for Fake {
    fn kubectl(&self, args: &[&str]) -> Result<String, String> {
        self.calls.lock().unwrap().push(args.join(" "));
        if args.contains(&"top") {
            return Ok(self.top.clone());
        }
        if args.contains(&"logs") {
            return Ok(self.log.clone());
        }
        self.pods.clone()
    }
}

#[test]
fn cgroup_v2_systemd_style_gives_pod_and_container() {
    let r = parse_cgroup(&v2(&UID.replace('-', "_"), CID)).unwrap();
    assert_eq!(r, CgRef { pod_uid: UID.into(), container_id: Some(CID.into()) });
}

#[test]
fn cgroup_pod_uid_with_dashes_or_underscores_is_the_same_pod() {
    assert_eq!(parse_cgroup(&v2(UID, CID)).unwrap().pod_uid, parse_cgroup(&v2(&UID.replace('-', "_"), CID)).unwrap().pod_uid);
}

#[test]
fn cgroup_v1_and_cgroupfs_styles_are_read() {
    let v1 = format!("12:memory:/kubepods/burstable/pod{UID}/{CID}\n1:name=systemd:/kubepods/burstable/pod{UID}/{CID}\n");
    assert_eq!(parse_cgroup(&v1).unwrap().container_id.as_deref(), Some(CID));
    let guaranteed = format!("0::/kubepods.slice/kubepods-pod{}.slice/cri-containerd-{CID}.scope", UID.replace('-', "_"));
    assert_eq!(parse_cgroup(&guaranteed).unwrap().pod_uid, UID);
    let sandbox = format!("0::/kubepods.slice/kubepods-besteffort.slice/kubepods-besteffort-pod{}.slice", UID.replace('-', "_"));
    assert_eq!(parse_cgroup(&sandbox).unwrap().container_id, None);
}

#[test]
fn unknown_cgroup_paths_give_no_label() {
    for t in ["", "0::/system.slice/sshd.service\n", "0::/user.slice/user-1000.slice/session-3.scope\n", "0::/kubepods.slice/kubepods-burstable.slice\n", "0::/kubepods.slice/kubepods-burstable-podnot_a_uid.slice/x\n"] {
        assert!(parse_cgroup(t).is_none(), "{t}");
    }
    let table = PodTable::parse(&pods_json());
    assert!(table.label("0::/system.slice/sshd.service").is_none());
    // a pod the table does not know
    assert!(table.label(&v2("99999999_9999_4999_8999_999999999999", CID)).is_none());
}

#[test]
fn the_table_labels_a_container_process_with_service_and_limits() {
    let table = PodTable::parse(&pods_json());
    assert_eq!(table.len(), 2);
    let l = table.label(&v2(&UID.replace('-', "_"), CID)).unwrap();
    assert_eq!((l.ns.as_str(), l.pod.as_str(), l.service.as_str(), l.container.as_str()), ("iyakki-data", "clickhouse-0", "clickhouse", "clickhouse"));
    assert_eq!(l.res, Res { cpu_req: Some(250), cpu_lim: Some(1000), mem_req: Some(256 << 20), mem_lim: Some(1 << 30) });
    assert_eq!(l.text(), "clickhouse (iyakki-data)");
    assert!(res_text(&l.res).contains("cpu 250m/1"));
    // no label on the pod: the service is the deployment's name, before the hash
    let g = table.label("0::/kubepods/pod11111111-2222-4333-8444-555555555555/abcdef0123456789").unwrap();
    assert_eq!(g.service, "gitea");
}

#[test]
fn quantities_are_read() {
    assert_eq!(parse_millicores("500m"), Some(500));
    assert_eq!(parse_millicores("2"), Some(2000));
    assert_eq!(parse_bytes("512Mi"), Some(512 << 20));
    assert_eq!(parse_bytes("1G"), Some(1_000_000_000));
    assert_eq!(parse_bytes("129e6"), Some(129_000_000));
    assert_eq!(parse_bytes("lots"), None);
}

#[test]
fn no_cluster_gives_no_labels_and_no_error() {
    assert!(fetch_table(&*Fake::down()).is_empty());
    assert!(PodTable::parse("not json").is_empty());
    assert!(PodTable::parse("{}").label(&v2(UID, CID)).is_none());
    let c = PodCache::new();
    c.refresh_now(&*Fake::up(), 100);
    assert_eq!(c.table().len(), 2);
    // the cluster is stopped: the labels go away
    assert!(!c.plan(false, 101));
    assert!(c.table().is_empty());
}

#[test]
fn the_cache_reads_at_most_every_fifteen_seconds() {
    let c = PodCache::new();
    assert!(c.plan(true, 100));
    assert!(!c.plan(true, 100), "a read is under way");
    c.refresh_now(&*Fake::up(), 100);
    assert!(!c.plan(true, 110));
    assert!(c.plan(true, 116));
}

fn proc(pid: u32, rss: u64, cgroup: &str) -> Proc {
    Proc { pid, ppid: 1, uid: 101, user: "101".into(), name: "java".into(), cmd: "java -jar x.jar --password hunter2 token=abc".into(), state: 'S', threads: 1, ticks: 0, rss, start_ticks: 0, cgroup: cgroup.into() }
}

#[test]
fn near_its_limit_and_grouping() {
    let table = PodTable::parse(&pods_json());
    let cg = v2(&UID.replace('-', "_"), CID);
    let procs = vec![proc(10, 980 << 20, &cg), proc(11, 10 << 20, &cg), proc(12, 5 << 20, ""), proc(13, 6 << 20, "")];
    let cpu: HashMap<u32, f32> = [(10, 20.0), (11, 5.0), (12, 1.0)].into();
    let r = rows(&procs, &cpu, &table);
    assert!(r[0].near && !r[1].near && !r[2].near);
    let g = groups(&r);
    assert_eq!(g.len(), 2);
    assert_eq!((g[0].name.as_str(), g[0].count, g[0].near), ("clickhouse (iyakki-data)", 2, true));
    assert_eq!((g[1].name.as_str(), g[1].count), ("host", 2));
    assert!(near_limit(1, 95.0, &Res { cpu_lim: Some(1000), ..Res::default() }));
    assert!(!near_limit(1, 50.0, &Res { cpu_lim: Some(1000), ..Res::default() }));
    assert!(!near_limit(u64::MAX / 2, 500.0, &Res::default()), "no limit set: never near");
}

#[test]
fn redaction_hides_secrets_in_command_lines_and_logs() {
    let r = redact("java -jar x.jar --password hunter2 token=abc user=bob");
    assert!(!r.contains("hunter2") && !r.contains("abc"), "{r}");
    assert!(r.contains("user=bob"));
    let g = redact("db password=hunter2 connecting");
    assert!(!g.contains("hunter2"));
}

#[test]
fn the_top_text_is_capped_labelled_and_redacted() {
    let table = PodTable::parse(&pods_json());
    let cg = v2(&UID.replace('-', "_"), CID);
    let procs: Vec<Proc> = (0..200).map(|i| proc(100 + i, 1 << 20, if i == 0 { &cg } else { "" })).collect();
    let cpu: HashMap<u32, f32> = [(100, 99.0)].into();
    let t = top_text(&rows(&procs, &cpu, &table), false, 500, false, true);
    assert_eq!(t.lines().filter(|l| l.starts_with("- ")).count(), MAX_ROWS);
    assert!(t.contains("clickhouse (iyakki-data) pod clickhouse-0") && t.contains("NEAR ITS LIMIT") || t.contains("clickhouse (iyakki-data)"));
    assert!(!t.contains("hunter2"));
    assert!(t.chars().count() <= MAX_OUT);
    let none = top_text(&rows(&procs, &cpu, &PodTable::empty()), true, 3, false, false);
    assert!(none.contains("No service names") && none.contains("host"));
    let grouped = top_text(&rows(&procs, &cpu, &table), true, 5, true, true);
    assert!(grouped.contains("clickhouse (iyakki-data): 1 process") && grouped.contains("host: 199 processes"));
}

#[test]
fn what_is_it_busy_with_shows_top_and_a_redacted_capped_log() {
    let f = Fake::up();
    let table = fetch_table(&*f);
    let t = busy_for(&*f, &table, "", "clickhouse-0").unwrap();
    assert!(t.contains("1020m") && t.contains("start") && t.contains("[hidden]") && !t.contains("hunter2"), "{t}");
    let calls = f.calls.lock().unwrap().join("|");
    assert!(calls.contains("top pod clickhouse-0") && calls.contains("--tail=40") && calls.contains("--limit-bytes="));
    // only reads
    assert!(!calls.contains("delete") && !calls.contains("exec"));
    assert!(busy_for(&*f, &table, "", "nope").is_err());
    assert!(busy_for(&*Fake::down(), &PodTable::empty(), "", "clickhouse-0").is_err());
    assert!(busy_text(&*f, "ns;rm", "x").contains("not a namespace"));
    let big = Arc::new(Fake { log: "x".repeat(50_000), ..Fake { calls: Mutex::new(vec![]), pods: Ok(pods_json()), top: String::new(), log: String::new() } });
    assert!(busy_text(&*big, "iyakki-data", "clickhouse-0").chars().count() <= MAX_OUT);
}

fn machine(tag: &str) -> SysRoots {
    let d = std::env::temp_dir().join(format!("iyakki-sysprocs-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let proc = d.join("proc");
    for (pid, cg) in [(5u32, v2(&UID.replace('-', "_"), CID)), (6, "0::/system.slice/x.service\n".into())] {
        let p = proc.join(pid.to_string());
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("stat"), format!("{pid} (java) S 1 0 0 0 0 0 0 0 0 0 10 0 0 0 20 0 1 0 5 1 {} 0\n", pid * 1000)).unwrap();
        std::fs::write(p.join("status"), "Uid:\t101\t101\t101\t101\n").unwrap();
        std::fs::write(p.join("cmdline"), "java\0--password\0hunter2\0").unwrap();
        std::fs::write(p.join("cgroup"), cg).unwrap();
    }
    std::fs::write(d.join("passwd"), "").unwrap();
    SysRoots { proc, sys: d.join("sys"), passwd: d.join("passwd") }
}

#[test]
fn the_chat_tools_work_with_and_without_a_cluster() {
    let roots = machine("tool");
    let up: Arc<dyn Kubectl> = Fake::up();
    let t = call_with("system_processes", &json!({"sort": "memory", "limit": 5}), &roots, &up, true).unwrap();
    assert!(t.contains("clickhouse (iyakki-data)") && !t.contains("hunter2") && t.contains("host"), "{t}");
    let down: Arc<dyn Kubectl> = Fake::down();
    let t = call_with("system_processes", &json!({}), &roots, &down, false).unwrap();
    assert!(t.contains("No service names") && !t.contains("clickhouse"));
    assert!(call_with("system_pod_busy", &json!({"pod": "clickhouse-0"}), &roots, &up, true).unwrap().contains("1020m"));
    assert!(call_with("system_pod_busy", &json!({"pod": "clickhouse-0"}), &roots, &down, false).is_err());
}

#[test]
fn the_tools_are_read_only_and_for_administrators() {
    use iyakki_sec::policy::Role;
    let admin = defs(&Role::Admin.template());
    assert_eq!(admin.len(), 2);
    assert!(admin.iter().all(|d| d.risk == Risk::Safe));
    assert!(defs(&Role::User.template()).is_empty());
}
