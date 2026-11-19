//! The assistant's window on dashboards: the tool definitions and the wrapper that binds them to the open dashboard.
//! The environment, the calls and the rules live in `iyakki_dash::tools`; this file keeps what needs the agent (`Tools`, `Services`).

use std::path::PathBuf;
use std::sync::Arc;

use iyakki_sec::policy::Policy;
use serde_json::Value;

pub use iyakki_dash::tools::*;
use super::cluster::Kubectl;
use crate::services::Services;

impl DashHost for Services {
    fn data_dir(&self) -> PathBuf {
        self.dir().to_path_buf()
    }
    fn home_root(&self, user: &str) -> PathBuf {
        crate::scope::home_root(user, self.dir())
    }
    fn resolve_secret(&self, user: &str, reference: &str) -> Result<zeroize::Zeroizing<String>, String> {
        match reference.split_once(':') {
            Some(("uvault", n)) => self.user_get(user, n).map_err(|e| e.to_string()),
            Some(("vault" | "ext", _)) => self.secrets.resolve(reference, None).map_err(|e| e.to_string()),
            _ => Err("only vault references are accepted".into()),
        }
    }
    fn vault_names(&self, user: &str) -> Vec<String> {
        self.user_names(user).unwrap_or_default()
    }
    fn vault_set(&self, user: &str, name: &str, value: &str) -> Result<(), String> {
        self.user_set(user, name, value).map_err(|e| e.to_string())
    }
    fn read_ok(&self, policy: &Policy, user: &str, path: &std::path::Path) -> bool {
        crate::scope::file_roots(policy, user, self.dir()).read_ok(policy, user, path)
    }
    fn kubectl(&self) -> Arc<dyn super::cluster::Kubectl> {
        self.k3sdev.clone()
    }
    fn runner(&self) -> Arc<dyn crate::wsexec::Runner> {
        Arc::new(crate::wsexec::System)
    }
    fn now_unix(&self) -> i64 {
        crate::clock::now_unix()
    }
}


impl Kubectl for crate::k3sdev::K3sDev {
    fn kubectl(&self, args: &[&str]) -> Result<String, String> {
        self.checked(args, crate::k3sdev::KUBECTL_CAP)
    }
    fn kubectl_big(&self, args: &[&str]) -> Result<String, String> {
        self.checked(args, crate::k3sdev::KUBECTL_BIG_CAP)
    }
}

impl crate::k3sdev::K3sDev {
    fn checked(&self, args: &[&str], cap: usize) -> Result<String, String> {
        use crate::k3sdev::Svc;
        match self.state() {
            Svc::NotInstalled => return Err("k3s is not installed by IYAKKI on this machine".into()),
            Svc::Running => {}
            other => return Err(format!("k3s is {}", other.word())),
        }
        let out = self.kubectl_cap(args, cap);
        super::cluster::kubectl_result(&out)
    }
}


/// The ordinary chat's tools: the standalone set.
pub fn standalone_call(name: &str, args: &Value, svc: &Arc<Services>, p: &Policy, user: &str) -> Result<String, String> {
    Env::from_host(svc, user, p).call(name, args, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dash::connector::*;
    use crate::dash::model::*;
    use crate::dash::multi::FetchCache;
    use crate::chat::{ChatBackend, Message, ModelTurn, ToolCall};
    use crate::dash::connector::fake::*;
    use crate::dash::session::shared;
    use crate::wsexec::FakeRunner;
    use iyakki_sec::policy::Role;
    use std::sync::Mutex;

    const NOW: i64 = 1_792_069_200_000;
    const SECRET: &str = "S3CRET-DO-NOT-LEAK";
    const HOST: &str = "http://prom.internal.example:9090";

    fn spike_matrix() -> String {
        let pts: Vec<String> = (0..60).map(|i| format!("[{},\"{}\"]", 1_792_065_600 + i * 60, if (40..43).contains(&i) { 900 } else { 100 + i % 3 })).collect();
        format!(r#"{{"status":"success","data":{{"resultType":"matrix","result":[{{"metric":{{"__name__":"latency_ms","service":"checkout"}},"values":[{}]}}]}}}}"#, pts.join(","))
    }

    fn env(tag: &str, up: &Arc<FakeUp>) -> Env {
        let dir = std::env::temp_dir().join(format!("iyakki-dashtools-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut deps = deps(up, &[("uvault:svc.prom", SECRET)], Arc::new(FakeRunner::new(&[])));
        deps.work_dir = dir.clone();
        let e = Env { user: "amy".into(), policy: Role::Admin.template(), data_dir: dir.clone(), home: dir.join("home"), deps, now_ms: Arc::new(|| NOW), vault_names: Arc::new(Vec::new), vault_set: Arc::new(|_, _| Ok(())), audit: Arc::new(|_, _| {}), cache: FetchCache::new(), people: Arc::new(AnyPeople) };
        let mut c = cfg("prometheus", HOST);
        c.id = "prom".into();
        c.name = "Prod Prometheus".into();
        c.values.insert("auth".into(), "bearer".into());
        c.secrets.insert("token".into(), "uvault:svc.prom".into());
        e.sources().save(c).unwrap();
        e
    }

    fn up() -> Arc<FakeUp> {
        FakeUp::new().on("query_range", 200, &spike_matrix()).on("label/__name__/values", 200, r#"{"status":"success","data":["latency_ms","up"]}"#).on("/api/v1/query?", 200, r#"{"status":"success","data":{"resultType":"vector","result":[]}}"#)
    }

    fn open(_e: &Env) -> Shared {
        shared(Dashboard::new("checkout", "Checkout"))
    }

    #[test]
    fn sources_are_listed_by_name_and_kind_only() {
        let e = env("src", &up());
        let t = e.call("dash_sources", &json!({}), None).unwrap();
        assert!(t.contains("Prod Prometheus") && t.contains("prometheus") && t.contains("PromQL"), "{t}");
        assert!(!t.contains("internal.example") && !t.contains(SECRET) && !t.contains("svc.prom"), "{t}");
        let st = open(&e);
        let ctx = DashTools::context(&e, &st);
        assert!(ctx.contains("Prod Prometheus [prometheus, PromQL]") && !ctx.contains("internal.example") && !ctx.contains(SECRET) && !ctx.contains("uvault"), "{ctx}");
        assert!(ctx.chars().count() <= crate::dash::session::CONTEXT_CHARS);
    }

    #[test]
    fn describe_lists_metrics_and_query_shows_the_query_and_a_table() {
        let u = up();
        let e = env("q", &u);
        let d = e.call("dash_describe", &json!({"source": "Prod Prometheus"}), None).unwrap();
        assert!(d.contains("- latency_ms") && d.contains("- up"), "{d}");
        let q = e.call("dash_query", &json!({"source": "prom", "query": "latency_ms{service=\"checkout\"}", "range": "1h"}), None).unwrap();
        assert!(q.contains("Query used on Prod Prometheus (PromQL)") && q.contains("latency_ms{service=\"checkout\"}") && q.contains("| series | points |") && q.contains("| 60 |"), "{q}");
        // the secret travelled in a header only
        let seen = u.seen.lock().unwrap().clone();
        assert!(seen.iter().all(|s| !s.1.contains(SECRET) && !s.3.contains(SECRET)));
        assert!(seen.iter().any(|s| s.2.iter().any(|h| h.1 == format!("Bearer {SECRET}"))));
        assert!(!q.contains(SECRET) && !d.contains(SECRET));
        assert!(e.call("dash_query", &json!({"source": "nope", "query": "up"}), None).is_err());
        assert!(e.call("dash_query", &json!({"source": "prom", "query": "up", "range": "forever"}), None).unwrap_err().contains("not a range"));
    }

    #[test]
    fn explain_quotes_real_numbers_from_the_frame() {
        let e = env("x", &up());
        let t = e.call("dash_explain", &json!({"source": "prom", "query": "latency_ms", "range": "1h", "unit": "ms", "compare": "previous"}), None).unwrap();
        assert!(t.contains("max 900 ms at 2026-10-15 12:40:00"), "{t}");
        assert!(t.contains("Peaks: 900 ms"), "{t}");
        assert!(t.contains("Earlier window") && t.contains("vs"), "{t}");
        let st = open(&e);
        assert!(e.call("dash_explain", &json!({"panel": "p9"}), Some(&st)).unwrap_err().contains("no panel"));
        assert!(e.call("dash_explain", &json!({"panel": "p1"}), None).is_err());
    }

    #[test]
    fn a_panel_is_previewed_added_highlighted_and_undone() {
        let e = env("add", &up());
        let st = open(&e);
        let args = json!({"title": "Checkout latency", "source": "Prod Prometheus", "query": "latency_ms{service=\"checkout\"}", "visual": "line", "unit": "ms", "legend_format": "{{service}}", "thresholds": [{"value": 500, "color": "red"}]});
        let r = e.call("dash_panel_add", &args, Some(&st)).unwrap();
        assert!(r.contains("Added panel p1") && r.contains("Preview: 1 series") && r.contains("Ctrl+Z"), "{r}");
        {
            let g = st.lock().unwrap();
            let p = g.dashboard.panel("p1").unwrap();
            assert_eq!((p.source.as_str(), p.options.unit, p.visual), ("prom", Unit::Millis, Visual::Line));
            assert_eq!(p.options.thresholds[0].color, [255, 100, 110]);
            assert_eq!(g.highlight.as_deref(), Some("p1"));
            assert!(g.dirty());
        }
        e.call("dash_panel_update", &json!({"id": "p1", "title": "Latency", "width": 24}), Some(&st)).unwrap();
        assert_eq!(st.lock().unwrap().dashboard.panel("p1").unwrap().grid.w, 24);
        e.call("dash_layout", &json!({"id": "p1", "height": 10}), Some(&st)).unwrap();
        e.call("dash_set_range", &json!({"from": "now-6h", "refresh_seconds": 30}), Some(&st)).unwrap();
        assert_eq!(st.lock().unwrap().dashboard.time.from, "now-6h");
        assert!(e.call("dash_set_range", &json!({"from": "yesterday-ish"}), Some(&st)).is_err());
        assert!(e.call("dash_set_range", &json!({"refresh_seconds": 7}), Some(&st)).is_err());
        e.call("dash_panel_remove", &json!({"id": "p1"}), Some(&st)).unwrap();
        assert!(st.lock().unwrap().dashboard.panels.is_empty());
        // Ctrl+Z x5 brings everything back and finally nothing
        let mut g = st.lock().unwrap();
        assert!(g.undo());
        assert_eq!(g.dashboard.panels.len(), 1);
        while g.undo() {}
        assert!(g.dashboard.panels.is_empty() && g.dashboard.time.from == "now-1h");
    }

    #[test]
    fn a_failing_or_writing_query_is_not_added() {
        let u = FakeUp::new().on("query_range", 400, r#"{"status":"error","errorType":"bad_data","error":"unexpected end of input"}"#);
        let e = env("bad", &u);
        let st = open(&e);
        let r = e.call("dash_panel_add", &json!({"title": "x", "source": "prom", "query": "rate("}), Some(&st)).unwrap_err();
        assert!(r.contains("the query did not work") && r.contains("unexpected end of input"), "{r}");
        assert!(st.lock().unwrap().dashboard.panels.is_empty());
        // SQL sources refuse writes before anything is sent
        let sql_up = FakeUp::new();
        let e2 = env("w", &sql_up);
        let mut c = cfg("clickhouse", "http://ch.internal:8123");
        c.id = "ch".into();
        c.name = "ClickHouse".into();
        e2.sources().save(c).unwrap();
        for q in ["DROP TABLE logs", "INSERT INTO t VALUES (1)", "SELECT 1; DELETE FROM t"] {
            let r = e2.call("dash_query", &json!({"source": "ch", "query": q}), None).unwrap_err();
            assert!(r.contains("only read") || r.contains("Dashboards only read"), "{q}: {r}");
            let r = e2.call("dash_panel_add", &json!({"title": "x", "source": "ch", "query": q, "visual": "table"}), Some(&open(&e2))).unwrap_err();
            assert!(r.contains("did not work"), "{r}");
        }
        assert!(sql_up.urls().is_empty(), "nothing reached the server");
    }

    #[test]
    fn the_ordinary_chat_saves_into_a_named_dashboard_and_open_only_tools_need_a_dashboard() {
        let e = env("solo", &up());
        let r = e.call("dash_panel_add", &json!({"title": "Latency", "source": "prom", "query": "latency_ms", "dashboard": "Checkout service"}), None).unwrap();
        assert!(r.contains("Saved panel p1") && r.contains("Checkout service"), "{r}");
        let d = e.dashboards().find("checkout service").unwrap();
        assert_eq!(d.panels.len(), 1);
        e.call("dash_panel_add", &json!({"title": "Again", "source": "prom", "query": "up", "dashboard": "Checkout service"}), None).unwrap();
        assert_eq!(e.dashboards().find("Checkout service").unwrap().panels.len(), 2);
        for t in ["dash_panel_update", "dash_panel_remove", "dash_set_range", "dash_layout", "dash_save"] {
            assert!(e.call(t, &json!({"id": "p1"}), None).unwrap_err().contains("no dashboard is open"), "{t}");
        }
        // the definitions: ask-level add outside, safe edits inside, save always asks
        let out: Vec<(String, Risk)> = defs(&e.policy, false).into_iter().map(|d| (d.spec.name, d.risk)).collect();
        assert!(out.contains(&("dash_panel_add".to_string(), Risk::Ask)) && !out.iter().any(|d| d.0 == "dash_save"));
        let inn: Vec<(String, Risk)> = defs(&e.policy, true).into_iter().map(|d| (d.spec.name, d.risk)).collect();
        assert!(inn.contains(&("dash_panel_add".to_string(), Risk::Safe)) && inn.contains(&("dash_save".to_string(), Risk::Ask)) && inn.contains(&("dash_layout".to_string(), Risk::Safe)));
        for t in ["dash_sources", "dash_describe", "dash_query", "dash_explain"] {
            assert!(inn.contains(&(t.to_string(), Risk::Safe)), "{t}");
        }
        assert!(!inn.iter().any(|d| d.0.contains("source_add") || d.0.contains("secret") || d.0.contains("connector")), "no tool creates connectors or credentials");
    }

    #[test]
    fn save_writes_the_file_and_clears_the_dirty_mark() {
        let e = env("save", &up());
        let st = open(&e);
        e.call("dash_panel_add", &json!({"title": "A", "source": "prom", "query": "up", "visual": "stat"}), Some(&st)).unwrap();
        assert!(st.lock().unwrap().dirty());
        e.call("dash_save", &json!({}), Some(&st)).unwrap();
        assert!(!st.lock().unwrap().dirty());
        assert_eq!(e.dashboards().load("checkout").unwrap().panels.len(), 1);
    }

    #[test]
    fn guests_and_blocked_hosts_get_nothing() {
        let e = env("pol", &up());
        let mut guest = e.clone();
        guest.policy = Role::Guest.template();
        assert!(defs(&guest.policy, true).is_empty());
        assert!(guest.call("dash_sources", &json!({}), None).is_err());
        let mut limited = e.clone();
        limited.policy.net_hosts = vec!["other.example".into()];
        let err = limited.call("dash_query", &json!({"source": "prom", "query": "up"}), None).unwrap_err();
        assert!(err.contains("may not reach prom.internal.example"), "{err}");
    }

    struct Script(Mutex<std::collections::VecDeque<ModelTurn>>, Mutex<Vec<String>>);
    impl ChatBackend for Script {
        fn complete(&self, _: &[Message]) -> Result<String, String> {
            Err("unused".into())
        }
        fn complete_tools(&self, h: &[Message], _t: &[crate::chat::ToolSpec]) -> Result<ModelTurn, String> {
            self.1.lock().unwrap().push(h.iter().map(|m| m.content.clone()).collect::<Vec<_>>().join("\n"));
            self.0.lock().unwrap().pop_front().ok_or_else(|| "script exhausted".to_string())
        }
    }
    struct NoTools;
    impl Tools for NoTools {
        fn defs(&self) -> Vec<ToolDef> {
            vec![tool("read_file", "read", json!({"type": "object"}), Risk::Safe, true)]
        }
        fn call(&self, n: &str, _: &Value) -> Result<String, String> {
            Ok(format!("{n} ran"))
        }
        fn describe(&self, n: &str, _: &Value) -> String {
            n.into()
        }
    }

    #[test]
    fn a_scripted_model_builds_a_panel_from_a_question_without_asking() {
        let e = Arc::new(env("run", &up()));
        let st = open(&e);
        let tc = |id: &str, n: &str, a: Value| ToolCall { id: id.into(), name: n.into(), args: a };
        let script = Arc::new(Script(
            Mutex::new(
                vec![
                    ModelTurn { text: String::new(), calls: vec![tc("1", "dash_sources", json!({}))] },
                    ModelTurn { text: String::new(), calls: vec![tc("2", "dash_describe", json!({"source": "prom"}))] },
                    ModelTurn { text: String::new(), calls: vec![tc("3", "dash_query", json!({"source": "prom", "query": "latency_ms", "range": "1h"}))] },
                    ModelTurn { text: String::new(), calls: vec![tc("4", "dash_panel_add", json!({"title": "Latency, last hour", "source": "prom", "query": "latency_ms", "visual": "area", "unit": "ms"}))] },
                    ModelTurn { text: "Added the panel: query `latency_ms`; it peaked at 900 ms at 12:40.".into(), calls: vec![] },
                ]
                .into(),
            ),
            Mutex::new(vec![]),
        ));
        let tools: Arc<dyn Tools> = Arc::new(DashTools::new(Arc::new(NoTools), e.clone(), st.clone()));
        let names: Vec<String> = tools.defs().into_iter().map(|d| d.spec.name).collect();
        assert!(names.contains(&"read_file".to_string()) && names.iter().filter(|n| *n == "dash_query").count() == 1);
        let h = crate::agent::start(script.clone(), tools, vec![Message::user("show checkout latency for the last hour")], crate::agent::RunLimits::default());
        let mut approvals = 0;
        let mut last = None;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            match h.events.recv_timeout(std::time::Duration::from_millis(50)) {
                Ok(crate::agent::RunEvent::Approval { .. }) => approvals += 1,
                Ok(ev @ crate::agent::RunEvent::Final { .. }) => {
                    last = Some(ev);
                    break;
                }
                Ok(crate::agent::RunEvent::Failed { error, .. }) => panic!("{error}"),
                _ => {}
            }
        }
        assert!(last.is_some(), "the run finished");
        assert_eq!(approvals, 0, "panel edits do not ask");
        let g = st.lock().unwrap();
        assert_eq!(g.dashboard.panels.len(), 1);
        assert_eq!(g.dashboard.panels[0].visual, Visual::Area);
        assert_eq!(g.highlight.as_deref(), Some("p1"));
        // what the model saw never held the secret or the address
        let seen = script.1.lock().unwrap().join("\n");
        assert!(!seen.contains(SECRET) && !seen.contains("internal.example"), "the model never sees credentials or addresses");
        assert!(seen.contains("Query used on Prod Prometheus"));
    }

    #[test]
    fn a_save_asks_even_inside_the_dashboard() {
        let e = Arc::new(env("ask", &up()));
        let t = DashTools::new(Arc::new(NoTools), e, open(&env("ask2", &up())));
        let risk = |n: &str| t.defs().into_iter().find(|d| d.spec.name == n).map(|d| d.risk);
        assert_eq!(risk("dash_save"), Some(Risk::Ask));
        assert_eq!(risk("dash_panel_add"), Some(Risk::Safe));
        assert_eq!(t.call("read_file", &json!({})).unwrap(), "read_file ran");
        assert!(describe("dash_panel_add", &json!({"title": "T", "visual": "line", "source": "s"})).contains("add the panel"));
    }

    #[test]
    fn a_typed_secret_goes_straight_to_the_vault_and_only_the_reference_is_kept() {
        let mut e = env("vault", &up());
        let stored = Arc::new(std::sync::Mutex::new(Vec::<(String, String)>::new()));
        let audit = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let (s2, a2) = (stored.clone(), audit.clone());
        e.vault_set = Arc::new(move |n, v| {
            s2.lock().unwrap().push((n.to_string(), v.to_string()));
            Ok(())
        });
        e.audit = Arc::new(move |k, d| a2.lock().unwrap().push(format!("{k} {d}")));
        let r = e.create_vault_entry("prometheus", "typed-SECRET-value-1").unwrap();
        assert_eq!(r, "uvault:svc.prometheus");
        assert_eq!(stored.lock().unwrap().clone(), vec![("svc.prometheus".to_string(), "typed-SECRET-value-1".to_string())]);
        assert!(!r.contains("typed-SECRET") && audit.lock().unwrap().iter().all(|l| !l.contains("typed-SECRET")), "{:?}", audit.lock().unwrap());
        assert!(audit.lock().unwrap().iter().any(|l| l.contains("svc.prometheus")));
        assert_eq!(e.create_vault_entry("svc.x", "v").unwrap(), "uvault:svc.x");
        assert!(e.create_vault_entry("Bad Name!", "v").is_err());
        assert!(e.create_vault_entry("ok", "").unwrap_err().contains("type the secret"));
        e.vault_set = Arc::new(|_, _| Err("vault is locked".into()));
        assert!(e.create_vault_entry("ok", "v").unwrap_err().contains("locked"));
        // the saved source holds the reference only
        let mut c = cfg("prometheus", "http://x:9090");
        c.id = "p2".into();
        c.name = "p2".into();
        c.values.insert("auth".into(), "bearer".into());
        c.secrets.insert("token".into(), r);
        e.sources().save(c).unwrap();
        let raw = std::fs::read_to_string(e.data_dir.join("dash").join("sources-amy.json")).unwrap();
        assert!(raw.contains("uvault:svc.prometheus") && !raw.contains("typed-SECRET"));
    }

    #[test]
    fn a_panel_can_take_queries_from_two_sources_with_expressions_and_one_failing_part_does_not_block_it() {
        let u = FakeUp::new()
            .on("query_range", 200, &spike_matrix())
            .on("_search", 200, r#"{"hits":{"total":{"value":1},"hits":[]},"aggregations":{"t":{"buckets":[{"key":1792065605000,"doc_count":4}]}}}"#);
        let e = env("multi", &u);
        let mut es = cfg("elasticsearch", "http://es.internal:9200");
        es.id = "es".into();
        es.name = "Logs".into();
        e.sources().save(es).unwrap();
        let st = open(&e);
        let args = json!({"title": "Latency and errors", "source": "prom", "query": "latency_ms", "alias": "latency", "queries": [{"source": "Logs", "query": "{\"aggs\":{\"t\":{\"date_histogram\":{\"field\":\"@timestamp\",\"fixed_interval\":\"1m\"}}}}", "target": "logs-*", "alias": "errors", "axis": "right", "unit": "number"}], "expressions": ["C = B / A * 100", "align nearest 60s"]});
        let r = e.call("dash_panel_add", &args, Some(&st)).unwrap();
        assert!(r.contains("Added panel p1") && r.contains("Preview: 3 series"), "{r}");
        let g = st.lock().unwrap();
        let p = g.dashboard.panel("p1").unwrap();
        assert_eq!((p.queries.len(), p.expressions.len(), p.queries[0].query.axis), (1, 2, Axis::Right));
        assert_eq!(p.queries[0].source, "es");
        drop(g);
        // a bad expression or axis is refused with a sentence and nothing is added
        for bad in [json!({"expressions": ["C = A /"]}), json!({"expressions": ["C = Q + 1"]}), json!({"queries": [{"source": "Logs", "query": "{}", "axis": "up"}]}), json!({"queries": [{"source": "nope", "query": "x"}]})] {
            let mut a = args.clone();
            for (k, v) in bad.as_object().unwrap() {
                a[k] = v.clone();
            }
            assert!(e.call("dash_panel_add", &a, Some(&st)).is_err(), "{bad}");
        }
        assert_eq!(st.lock().unwrap().dashboard.panels.len(), 1);
        // one source down: the panel is still added, with the failure named
        let down = FakeUp::new().on("query_range", 200, &spike_matrix());
        let e2 = env("multi2", &down);
        let mut es2 = cfg("elasticsearch", "http://es.internal:9200");
        es2.id = "es".into();
        es2.name = "Logs".into();
        e2.sources().save(es2).unwrap();
        let st2 = open(&e2);
        let r = e2.call("dash_panel_add", &args, Some(&st2)).unwrap();
        assert!(r.contains("Added panel p1") && r.contains("Some parts failed") && r.contains("B (es)"), "{r}");
        let q = e2.call("dash_explain", &json!({"panel": "p1", "range": "1h"}), Some(&st2)).unwrap();
        assert!(q.contains("latency"), "{q}");
    }

    #[test]
    fn nothing_in_the_tools_audit_or_context_holds_the_secret_even_when_the_server_echoes_it() {
        struct Echo;
        impl iyakki_proxy::Upstream for Echo {
            fn request(&self, _m: &str, _u: &str, h: &[(String, String)], _b: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
                let auth = h.iter().find(|x| x.0 == "Authorization").map(|x| x.1.clone()).unwrap_or_default();
                Ok(iyakki_proxy::UpstreamResponse { status: 400, body: format!(r#"{{"status":"error","errorType":"bad_data","error":"you sent {auth}"}}"#).into_bytes() })
            }
        }
        struct T;
        impl Transport for T {
            fn upstream(&self, _: &NetOptions) -> Result<Arc<dyn iyakki_proxy::Upstream>, String> {
                Ok(Arc::new(Echo))
            }
        }
        let mut e = env("echo", &up());
        e.deps.transport = Arc::new(T);
        let log = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let l2 = log.clone();
        e.audit = Arc::new(move |k, d| l2.lock().unwrap().push(format!("{k} {d}")));
        let st = open(&e);
        let mut all = String::new();
        for (name, args) in [("dash_query", json!({"source": "prom", "query": "up"})), ("dash_describe", json!({"source": "prom"})), ("dash_explain", json!({"source": "prom", "query": "up"})), ("dash_panel_add", json!({"title": "x", "source": "prom", "query": "up"}))] {
            all.push_str(&format!("{:?}\n", e.call(name, &args, Some(&st))));
        }
        all.push_str(&DashTools::context(&e, &st));
        all.push_str(&log.lock().unwrap().join("\n"));
        all.push_str(&format!("{:?}", st.lock().unwrap().errors));
        assert!(all.contains("redacted"), "the echo came back cleaned: {all}");
        assert!(!all.contains(SECRET), "{all}");
        // the tools offer no way to read or write a vault entry or a source
        assert!(!defs(&e.policy, true).iter().any(|d| d.spec.name.contains("vault") || d.spec.name.contains("secret") || d.spec.name.contains("source_")));
    }

    #[test]
    fn ranges_parse() {
        let d = TimeRange::last(3_600_000, NOW);
        assert_eq!(parse_range(Some(&json!("6h")), d, NOW).unwrap(), TimeRange::last(21_600_000, NOW));
        assert_eq!(parse_range(None, d, NOW).unwrap(), d);
        let r = parse_range(Some(&json!({"from": "now-3h", "to": "now-2h"})), d, NOW).unwrap();
        assert_eq!((r.from_ms, r.to_ms), (NOW - 10_800_000, NOW - 7_200_000));
        assert!(parse_range(Some(&json!({"from": "blah"})), d, NOW).is_err());
        assert!(parse_range(Some(&json!("800d")), d, NOW).is_err());
        assert!(parse_range(Some(&json!(5)), d, NOW).is_err());
    }

#[test]
fn building_the_dash_environment_for_an_administrator_seeds_the_default_once_and_never_for_others() {
    use iyakki_proxy::{Upstream, UpstreamResponse};
    struct Dead;
    impl Upstream for Dead {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<UpstreamResponse, String> {
            Err("no network".into())
        }
    }
    let d = std::env::temp_dir().join(format!("iyakki-dashseed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let svc: Arc<Services> = crate::services::Services::open(&d, Arc::new(Dead), Arc::new(Dead)).unwrap();
    let admin = iyakki_sec::policy::Role::Admin.template();
    let user = iyakki_sec::policy::Role::User.template();
    let (root_home, sam_home) = (d.join("homes/root"), d.join("homes/sam"));
    let home = root_home.clone();
    let boards = crate::dash::store::Dashboards::open(&home);
    assert!(boards.load(crate::dash::seed::DASHBOARD_ID).is_err());
    let _ = Env::from_host_in(&(svc.clone() as Arc<dyn DashHost>), "root", &admin, root_home.clone());
    assert!(boards.load(crate::dash::seed::DASHBOARD_ID).is_ok(), "the administrator's first Env seeds the default dashboard");
    // a person who deleted it does not get it back on the next Env
    boards.delete(crate::dash::seed::DASHBOARD_ID).unwrap();
    let _ = Env::from_host_in(&(svc.clone() as Arc<dyn DashHost>), "root", &admin, root_home.clone());
    assert!(boards.load(crate::dash::seed::DASHBOARD_ID).is_err());
    // an ordinary account is not seeded
    let _ = Env::from_host_in(&(svc.clone() as Arc<dyn DashHost>), "sam", &user, sam_home.clone());
    assert!(crate::dash::store::Dashboards::open(&sam_home).load(crate::dash::seed::DASHBOARD_ID).is_err());
    let _ = std::fs::remove_dir_all(&d);
}

}

#[cfg(test)]
use serde_json::json;
#[cfg(test)]
use crate::agent::Tools;
#[cfg(test)]
use crate::agent::Risk;
#[cfg(test)]
use crate::agent::ToolDef;
#[cfg(test)]
use super::session::Shared;
