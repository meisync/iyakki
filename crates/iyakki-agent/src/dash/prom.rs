//! Prometheus (and anything that speaks its HTTP API: VictoriaMetrics, Mimir, Thanos, Cortex).
//! `/api/v1/query` for a single value, `/api/v1/query_range` for a chart, `/api/v1/label/__name__/values` and
//! `/api/v1/series` for browsing.

use std::collections::BTreeMap;

use serde_json::Value;

use super::connector::*;
use super::frame::*;
use super::readonly::prom_request_ok;

pub struct Prometheus {
    id: String,
    http: Http,
}

pub fn info() -> KindInfo {
    KindInfo {
        kind: "prometheus".into(),
        label: "Prometheus".into(),
        language: Lang::PromQl,
        about: "Metrics from Prometheus, VictoriaMetrics, Mimir or Thanos (their HTTP API).".into(),
        fields: net_fields("http://localhost:9090"),
        example: "http://localhost:9090".into(),
    }
}

impl Prometheus {
    pub fn new(cfg: &SourceConfig, d: &Deps) -> Result<Prometheus, DashError> {
        Ok(Prometheus { id: cfg.id.clone(), http: Http::from_config(cfg, d, &format!("Prometheus “{}”", cfg.name))? })
    }

    pub fn with_http(id: &str, http: Http) -> Prometheus {
        Prometheus { id: id.into(), http }
    }

    fn get(&self, path_q: &str, cancel: &Cancel) -> Result<Value, DashError> {
        prom_request_ok("GET", path_q).map_err(DashError::read_only)?;
        let r = match self.http.send("GET", path_q, &[], &[], cancel) {
            Ok(r) => r,
            Err(e) if e.kind == ErrKind::BadQuery => return Err(e),
            Err(e) => return Err(e),
        };
        let v = r.json()?;
        check_status(&v)?;
        Ok(v)
    }
}

/// `{"status":"error","errorType":"bad_data","error":"parse error ..."}` becomes a bad-query error with the server's words.
fn check_status(v: &Value) -> Result<(), DashError> {
    match v.get("status").and_then(Value::as_str) {
        Some("success") => Ok(()),
        Some(_) => {
            let t = v.get("errorType").and_then(Value::as_str).unwrap_or("error");
            let m = v.get("error").and_then(Value::as_str).unwrap_or("the query failed");
            let e = DashError::new(if t == "timeout" { ErrKind::Limit } else { ErrKind::BadQuery }, format!("Prometheus: {t}: {m}"));
            Err(if t == "timeout" { e.fix("The query took too long. Use a shorter range or a coarser rate window.") } else { e })
        }
        None => Err(DashError::parse("the answer has no `status` field")),
    }
}

/// The PromQL step in seconds as Prometheus likes it, and never so small that the range would give more than 11,000 points.
pub fn step_seconds(range: &TimeRange, step_ms: Option<i64>) -> f64 {
    let min = (range.len_ms() as f64 / MAX_POINTS as f64 / 1000.0).max(0.001);
    let s = step_ms.map_or_else(|| auto_step_ms(range.len_ms(), 600) as f64 / 1000.0, |m| m as f64 / 1000.0);
    s.max(min).max(0.001)
}

/// A readable step for about `max_points` points over `range_ms` (1s, 5s, 15s, 30s, 1m, ... 1d).
pub fn auto_step_ms(range_ms: i64, max_points: usize) -> i64 {
    const NICE: [i64; 17] = [1_000, 5_000, 10_000, 15_000, 30_000, 60_000, 120_000, 300_000, 600_000, 900_000, 1_800_000, 3_600_000, 7_200_000, 21_600_000, 43_200_000, 86_400_000, 604_800_000];
    let want = range_ms / max_points.max(1) as i64;
    *NICE.iter().find(|n| **n >= want).unwrap_or(&604_800_000)
}

fn labels_of(m: &Value) -> BTreeMap<String, String> {
    m.as_object().map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().map_or_else(|| v.to_string(), String::from))).collect()).unwrap_or_default()
}

/// `metric{a="b",c="d"}` (or just `{a="b"}` without a name).
pub fn series_name(l: &BTreeMap<String, String>) -> String {
    let name = l.get("__name__").cloned().unwrap_or_default();
    let rest: Vec<String> = l.iter().filter(|(k, _)| *k != "__name__").map(|(k, v)| format!("{k}=\"{v}\"")).collect();
    match (name.is_empty(), rest.is_empty()) {
        (true, true) => "value".into(),
        (false, true) => name,
        (_, false) => format!("{name}{{{}}}", rest.join(",")),
    }
}

fn point(p: &Value) -> Option<(i64, f64)> {
    let a = p.as_array()?;
    let t = (a.first()?.as_f64()? * 1000.0).round() as i64;
    let v = parse_f64(a.get(1)?.as_str()?)?;
    Some((t, v))
}

/// Turn the `data` of a Prometheus answer into a frame: a matrix into series, a vector into a table (and one-point series),
/// a scalar or string into a one-cell table.
pub fn parse_data(data: &Value, max_series: usize) -> Result<Frame, DashError> {
    let kind = data.get("resultType").and_then(Value::as_str).ok_or_else(|| DashError::parse("the answer has no resultType"))?;
    let res = data.get("result").ok_or_else(|| DashError::parse("the answer has no result"))?;
    let mut f = Frame::default();
    match kind {
        "matrix" => {
            let items = res.as_array().ok_or_else(|| DashError::parse("matrix result is not a list"))?;
            for it in items {
                let labels = labels_of(&it["metric"]);
                let points: Vec<(i64, f64)> = it["values"].as_array().map(|a| a.iter().filter_map(point).collect()).unwrap_or_default();
                f.series.push(Series { name: series_name(&labels), labels, points, axis: 0 });
            }
        }
        "vector" => {
            let items = res.as_array().ok_or_else(|| DashError::parse("vector result is not a list"))?;
            let mut label_keys: Vec<String> = vec![];
            for it in items {
                for k in labels_of(&it["metric"]).keys() {
                    if !label_keys.contains(k) {
                        label_keys.push(k.clone());
                    }
                }
            }
            // the metric name is in the series name; keep it as a column only when it varies
            let names: std::collections::BTreeSet<String> = items.iter().filter_map(|it| labels_of(&it["metric"]).get("__name__").cloned()).collect();
            if names.len() <= 1 {
                label_keys.retain(|k| k != "__name__");
            }
            f.columns = label_keys.iter().map(|k| Column { name: k.clone(), ty: ColType::Str }).collect();
            f.columns.push(Column { name: "value".into(), ty: ColType::Float });
            for it in items {
                let labels = labels_of(&it["metric"]);
                let pt = point(&it["value"]);
                let mut row: Vec<Value> = label_keys.iter().map(|k| labels.get(k).map_or(Value::Null, |v| Value::from(v.as_str()))).collect();
                row.push(pt.map_or(Value::Null, |p| num(p.1)));
                f.rows.push(row);
                if let Some(p) = pt {
                    f.series.push(Series { name: series_name(&labels), labels, points: vec![p], axis: 0 });
                }
            }
        }
        "scalar" => {
            let p = point(res);
            f.columns = vec![Column { name: "value".into(), ty: ColType::Float }];
            f.rows = vec![vec![p.map_or(Value::Null, |p| num(p.1))]];
            if let Some(p) = p {
                f.series.push(Series { name: "value".into(), labels: BTreeMap::new(), points: vec![p], axis: 0 });
            }
        }
        "string" => {
            f.columns = vec![Column { name: "value".into(), ty: ColType::Str }];
            f.rows = vec![vec![res.get(1).cloned().unwrap_or(Value::Null)]];
        }
        other => return Err(DashError::parse(format!("unknown result type `{other}`"))),
    }
    if f.series.len() > max_series {
        let n = f.series.len();
        f.series.truncate(max_series);
        f.meta.truncated = true;
        f.meta.notices.push(format!("{max_series} of {n} series are shown; add a label filter or topk()"));
    }
    if let Some(w) = data.get("warnings").and_then(Value::as_array) {
        f.meta.notices.extend(w.iter().filter_map(Value::as_str).map(String::from));
    }
    Ok(f)
}

impl DataSource for Prometheus {
    fn id(&self) -> &str {
        &self.id
    }
    fn kind(&self) -> &str {
        "prometheus"
    }
    fn language(&self) -> Lang {
        Lang::PromQl
    }

    fn test(&self, cancel: &Cancel) -> Result<TestInfo, DashError> {
        let v = self.get("/api/v1/status/buildinfo", cancel).or_else(|e| if e.kind == ErrKind::BadQuery { self.get("/api/v1/query?query=1", cancel) } else { Err(e) })?;
        let ver = v.pointer("/data/version").and_then(Value::as_str).unwrap_or("");
        Ok(TestInfo { ok: true, detail: if ver.is_empty() { "reachable".into() } else { format!("Prometheus {ver}") } })
    }

    fn browse(&self, path: &[String], cancel: &Cancel) -> Result<Vec<Node>, DashError> {
        match path {
            [] => {
                let v = self.get("/api/v1/label/__name__/values", cancel)?;
                let mut names: Vec<String> = v["data"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
                names.sort();
                names.truncate(MAX_ROWS);
                Ok(names.into_iter().map(|n| Node { name: n, kind: NodeKind::Metric, detail: String::new(), has_children: true }).collect())
            }
            [metric] => {
                let sel = format!("{{__name__=\"{}\"}}", metric.replace(['"', '\\'], ""));
                let v = self.get(&format!("/api/v1/series?match%5B%5D={}", enc(&sel)), cancel)?;
                let mut keys: BTreeMap<String, String> = BTreeMap::new();
                for s in v["data"].as_array().into_iter().flatten().take(500) {
                    for (k, val) in labels_of(s) {
                        if k != "__name__" {
                            keys.entry(k).or_insert(val);
                        }
                    }
                }
                Ok(keys.into_iter().map(|(k, sample)| Node { name: k, kind: NodeKind::Label, detail: format!("e.g. {sample}"), has_children: false }).collect())
            }
            _ => Ok(vec![]),
        }
    }

    fn query(&self, req: &QueryRequest, cancel: &Cancel) -> Result<Frame, DashError> {
        let started = std::time::Instant::now();
        let q = req.text.trim();
        if q.is_empty() {
            return Err(DashError::new(ErrKind::BadQuery, "the PromQL query is empty").fix("Type a metric name, for example up, or rate(node_cpu_seconds_total[5m])."));
        }
        let timeout = req.timeout_s.unwrap_or(DEFAULT_TIMEOUT_S).clamp(1, MAX_TIMEOUT_S);
        let path = match req.step_ms {
            Some(_) => {
                let step = step_seconds(&req.range, req.step_ms);
                format!("/api/v1/query_range?query={}&start={:.3}&end={:.3}&step={}&timeout={}s", enc(q), req.range.from_ms as f64 / 1000.0, req.range.to_ms as f64 / 1000.0, trim_num(step), timeout)
            }
            None => format!("/api/v1/query?query={}&time={:.3}&timeout={}s", enc(q), req.range.to_ms as f64 / 1000.0, timeout),
        };
        // long queries would not fit in a URL: send them as a form body (the same read-only call)
        let v = if path.len() > 6000 {
            let (p, form) = path.split_once('?').unwrap();
            prom_request_ok("POST", p).map_err(DashError::read_only)?;
            let r = self.http.send("POST", p, &[("Content-Type", "application/x-www-form-urlencoded")], form.as_bytes(), cancel)?;
            let v = r.json()?;
            check_status(&v)?;
            v
        } else {
            self.get(&path, cancel)?
        };
        let mut f = parse_data(&v["data"], req.limit().min(MAX_SERIES))?;
        f.cap(req.limit(), MAX_SERIES, MAX_POINTS);
        f.meta.took_ms = took(started);
        f.meta.executed = q.to_string();
        Ok(f)
    }

    fn starter(&self, target: &str) -> String {
        if target.ends_with("_total") || target.ends_with("_count") || target.ends_with("_sum") {
            format!("rate({target}[5m])")
        } else {
            target.to_string()
        }
    }
}

fn trim_num(v: f64) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dash::connector::fake::*;
    use crate::wsexec::FakeRunner;
    use std::sync::Arc;

    const MATRIX: &str = r#"{"status":"success","data":{"resultType":"matrix","result":[
        {"metric":{"__name__":"node_load1","instance":"a:9100","job":"node"},"values":[[1792065600,"0.5"],[1792065660,"0.75"],[1792065720,"NaN"],[1792065780,"+Inf"],[1792065840,"1.25"]]},
        {"metric":{"instance":"b:9100"},"values":[[1792065600,"2"]]}]}}"#;
    const VECTOR: &str = r#"{"status":"success","data":{"resultType":"vector","result":[
        {"metric":{"__name__":"up","job":"node","instance":"a"},"value":[1792069200.123,"1"]},
        {"metric":{"__name__":"up","job":"node","instance":"b"},"value":[1792069200.123,"0"]}]}}"#;

    fn src(up: &Arc<FakeUp>) -> Prometheus {
        let d = deps(up, &[], Arc::new(FakeRunner::new(&[])));
        Prometheus::new(&cfg("prometheus", "http://prom:9090/"), &d).unwrap()
    }

    #[test]
    fn a_matrix_becomes_series_with_nan_dropped() {
        let up = FakeUp::new().on("query_range", 200, MATRIX);
        let mut r = QueryRequest::new("node_load1", range());
        r.step_ms = Some(60_000);
        let f = src(&up).query(&r, &Cancel::new()).unwrap();
        assert_eq!(f.series.len(), 2);
        assert_eq!(f.series[0].name, "node_load1{instance=\"a:9100\",job=\"node\"}");
        assert_eq!(f.series[0].points, vec![(1_792_065_600_000, 0.5), (1_792_065_660_000, 0.75), (1_792_065_840_000, 1.25)]);
        assert_eq!(f.series[1].name, "{instance=\"b:9100\"}");
        let url = up.urls().remove(0);
        assert!(url.contains("start=1792065600.000") && url.contains("end=1792069200.000") && url.contains("step=60") && url.contains("query=node_load1"), "{url}");
        assert_eq!(f.meta.executed, "node_load1");
    }

    #[test]
    fn a_vector_becomes_a_table_and_single_point_series() {
        let up = FakeUp::new().on("/api/v1/query?", 200, VECTOR);
        let f = src(&up).query(&QueryRequest::new("up", range()), &Cancel::new()).unwrap();
        assert_eq!(f.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["instance", "job", "value"]);
        assert_eq!(f.rows.len(), 2);
        assert_eq!(f.cell(1, 2), "0");
        assert_eq!(f.series[0].points[0].0, 1_792_069_200_123);
        assert!(up.urls()[0].contains("time=1792069200.000"));
    }

    #[test]
    fn errors_carry_the_servers_words_and_a_fix() {
        let up = FakeUp::new().on("query", 400, r#"{"status":"error","errorType":"bad_data","error":"invalid parameter \"query\": 1:5: parse error: unexpected end of input"}"#);
        let e = src(&up).query(&QueryRequest::new("rate(", range()), &Cancel::new()).unwrap_err();
        assert_eq!(e.kind, ErrKind::BadQuery);
        assert!(e.message.contains("parse error: unexpected end of input"), "{e}");
        let up = FakeUp::new().on("query", 200, r#"{"status":"error","errorType":"timeout","error":"query timed out"}"#);
        let e = src(&up).query(&QueryRequest::new("x", range()), &Cancel::new()).unwrap_err();
        assert_eq!(e.kind, ErrKind::Limit);
        assert!(e.fix.is_some());
    }

    #[test]
    fn odd_json_never_panics() {
        for body in ["", "null", "[]", "{}", r#"{"status":"success"}"#, r#"{"status":"success","data":{"resultType":"matrix","result":[{"values":"x"},{"metric":5,"values":[[1],[null,"1"],["a"],[1,2]]}]}}"#, r#"{"status":"success","data":{"resultType":"vector","result":[1,"x",null]}}"#, r#"{"status":"success","data":{"resultType":"zzz","result":[]}}"#, "<html>login</html>"] {
            let up = FakeUp::new().on("query", 200, body);
            let _ = src(&up).query(&QueryRequest::new("x", range()), &Cancel::new());
        }
    }

    #[test]
    fn series_beyond_the_limit_are_cut_with_a_notice() {
        let items: Vec<String> = (0..30).map(|i| format!(r#"{{"metric":{{"i":"{i}"}},"values":[[1792065600,"1"]]}}"#)).collect();
        let body = format!(r#"{{"status":"success","data":{{"resultType":"matrix","result":[{}]}}}}"#, items.join(","));
        let up = FakeUp::new().on("query_range", 200, &body);
        let mut r = QueryRequest::new("x", range());
        r.step_ms = Some(60_000);
        r.limit = 10;
        let f = src(&up).query(&r, &Cancel::new()).unwrap();
        assert_eq!(f.series.len(), 10);
        assert!(f.meta.truncated && f.meta.notices[0].contains("10 of 30"), "{:?}", f.meta);
    }

    #[test]
    fn browse_lists_metrics_and_labels() {
        let up = FakeUp::new().on("label/__name__/values", 200, r#"{"status":"success","data":["up","node_load1","a_total"]}"#).on("/api/v1/series", 200, r#"{"status":"success","data":[{"__name__":"up","job":"node","instance":"a"}]}"#);
        let s = src(&up);
        let m = s.browse(&[], &Cancel::new()).unwrap();
        assert_eq!(m.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), vec!["a_total", "node_load1", "up"]);
        let l = s.browse(&["up".into()], &Cancel::new()).unwrap();
        assert_eq!(l.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), vec!["instance", "job"]);
        assert!(up.urls()[1].contains("match%5B%5D=%7B__name__%3D%22up%22%7D"), "{:?}", up.urls());
        assert_eq!(s.starter("http_requests_total"), "rate(http_requests_total[5m])");
    }

    #[test]
    fn test_reports_the_version_and_explains_failures() {
        let up = FakeUp::new().on("buildinfo", 200, r#"{"status":"success","data":{"version":"2.53.0"}}"#);
        assert_eq!(src(&up).test(&Cancel::new()).unwrap().detail, "Prometheus 2.53.0");
        let up = FakeUp::new();
        *up.fail.lock().unwrap() = Some("Connection Failed: Connection refused (os error 111)".into());
        let e = src(&up).test(&Cancel::new()).unwrap_err();
        assert_eq!(e.kind, ErrKind::Network);
        assert!(e.to_string().contains("connection refused") && e.to_string().contains("Nothing is listening"), "{e}");
        let up = FakeUp::new().on("buildinfo", 401, "unauthorized");
        assert_eq!(src(&up).test(&Cancel::new()).unwrap_err().kind, ErrKind::Auth);
    }

    #[test]
    fn step_math_stays_under_the_point_limit() {
        for len in [1_000i64, 60_000, 3_600_000, 86_400_000, 30 * 86_400_000, 400 * 86_400_000] {
            let r = TimeRange { from_ms: 0, to_ms: len };
            let s = step_seconds(&r, None);
            assert!(len as f64 / 1000.0 / s <= MAX_POINTS as f64 + 1.0, "{len}");
            let tiny = step_seconds(&r, Some(1));
            assert!(len as f64 / 1000.0 / tiny <= MAX_POINTS as f64 + 1.0);
        }
        assert_eq!(auto_step_ms(3_600_000, 600), 10_000);
        assert_eq!(auto_step_ms(86_400_000, 600), 300_000);
    }

    #[test]
    fn cancel_stops_before_sending() {
        let up = FakeUp::new();
        let c = Cancel::new();
        c.cancel();
        let e = src(&up).query(&QueryRequest::new("up", range()), &c).unwrap_err();
        assert_eq!(e.kind, ErrKind::Cancelled);
        assert!(up.urls().is_empty());
    }
}
