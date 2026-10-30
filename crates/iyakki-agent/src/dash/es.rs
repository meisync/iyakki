//! Elasticsearch and OpenSearch. A query is the search body (Query DSL as JSON) plus the index it runs on (`target`).
//! `_search` answers become a table (hits) or, for aggregations, a table / time series (date_histogram and terms buckets
//! with metrics inside). Browsing uses `_cat/indices` and `_mapping`.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use super::connector::*;
use super::frame::*;
use super::readonly::es_request_ok;

pub struct Elastic {
    id: String,
    http: Http,
}

pub fn info() -> KindInfo {
    let mut fields = net_fields("http://localhost:9200");
    fields.insert(3, Field::new("default_index", "Default index", FieldKind::Text, "index or pattern used when a panel names none, for example logs-*"));
    KindInfo {
        kind: "elasticsearch".into(),
        label: "Elasticsearch / OpenSearch".into(),
        language: Lang::EsDsl,
        about: "Search and aggregate documents with the Query DSL. Works with OpenSearch too. Use a read-only user.".into(),
        fields,
        example: "http://localhost:9200 (own server) or cluster://iyakki-data/elasticsearch:9200 (Quick install, runs inside the cluster)".into(),
    }
}

impl Elastic {
    pub fn new(cfg: &SourceConfig, d: &Deps) -> Result<Elastic, DashError> {
        Ok(Elastic { id: cfg.id.clone(), http: Http::from_config(cfg, d, &format!("Elasticsearch “{}”", cfg.name))? })
    }

    pub fn with_http(id: &str, http: Http) -> Elastic {
        Elastic { id: id.into(), http }
    }

    fn call(&self, method: &str, path: &str, body: &[u8], cancel: &Cancel) -> Result<Value, DashError> {
        es_request_ok(method, path).map_err(DashError::read_only)?;
        let ct = if body.is_empty() { vec![] } else { vec![("Content-Type", "application/json")] };
        let r = self.http.send(method, path, &ct, body, cancel)?;
        r.json()
    }
}

/// Index names go in the URL: letters, digits and `-_.*,:+` only.
pub fn valid_index(s: &str) -> bool {
    !s.is_empty() && s.len() <= 255 && !s.starts_with(['_', '-', '+']) && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.*,:+".contains(c))
}

/// A ready date_histogram query: documents (or a metric) per bucket over a time range, optionally split by a keyword field.
/// `filter` is a query_string ("status:500 AND service:checkout"), empty for everything.
pub fn date_histogram_query(time_field: &str, interval: &str, filter: &str, metric: Option<(&str, &str)>, split_by: Option<(&str, usize)>) -> String {
    let mut inner = json!({"date_histogram": {"field": time_field, "fixed_interval": interval, "min_doc_count": 0}});
    let metric_agg = metric.map(|(func, field)| json!({"m": {func: {"field": field}}}));
    let mut bucket = inner.as_object_mut().unwrap().clone();
    if let Some(m) = &metric_agg {
        bucket.insert("aggs".into(), m.clone());
    }
    let aggs = match split_by {
        Some((f, n)) => json!({"split": {"terms": {"field": f, "size": n}, "aggs": {"t": Value::Object(bucket)}}}),
        None => json!({"t": Value::Object(bucket)}),
    };
    let q = if filter.trim().is_empty() { json!({"match_all": {}}) } else { json!({"query_string": {"query": filter}}) };
    serde_json::to_string_pretty(&json!({
        "size": 0,
        "query": {"bool": {"filter": [q, {"range": {time_field: {"gte": "$__from", "lte": "$__to", "format": "epoch_millis"}}}]}},
        "aggs": aggs
    }))
    .unwrap_or_default()
}

/// A ready top-N query: the most common values of a keyword field.
pub fn terms_query(field: &str, size: usize, filter: &str) -> String {
    let q = if filter.trim().is_empty() { json!({"match_all": {}}) } else { json!({"query_string": {"query": filter}}) };
    serde_json::to_string_pretty(&json!({"size": 0, "query": q, "aggs": {"top": {"terms": {"field": field, "size": size}}}})).unwrap_or_default()
}

fn agg_kind(def: &Value) -> Option<&str> {
    def.as_object()?.keys().find(|k| *k != "aggs" && *k != "aggregations" && *k != "meta").map(String::as_str)
}

/// One flattened bucket row: the path of keys so far, and the metrics.
type Row = Vec<(String, Value)>;

fn metric_cells(name: &str, v: &Value, out: &mut Row) {
    if let Some(x) = v.get("value") {
        out.push((name.to_string(), x.clone()));
    } else if let Some(vs) = v.get("values").and_then(Value::as_object) {
        for (k, x) in vs {
            out.push((format!("{name} {k}"), x.clone()));
        }
    } else if let Some(o) = v.as_object() {
        // stats / extended_stats / boxplot style: numeric fields
        for (k, x) in o {
            if x.is_number() {
                out.push((format!("{name} {k}"), x.clone()));
            }
        }
    }
}

fn walk(defs: &Map<String, Value>, resp: &Map<String, Value>, prefix: &Row, rows: &mut Vec<Row>, depth: usize) {
    if depth > 6 || rows.len() > MAX_ROWS * 4 {
        return;
    }
    let mut metrics: Row = vec![];
    let mut bucketed = false;
    for (name, def) in defs {
        let Some(r) = resp.get(name) else { continue };
        let kind = agg_kind(def).unwrap_or("");
        let buckets = r.get("buckets");
        if let Some(b) = buckets {
            bucketed = true;
            let items: Vec<(Value, &Value)> = match b {
                Value::Array(a) => a.iter().map(|x| (x.get("key_as_string").or_else(|| x.get("key")).cloned().unwrap_or(Value::Null), x)).collect(),
                Value::Object(o) => o.iter().map(|(k, x)| (Value::from(k.as_str()), x)).collect(),
                _ => vec![],
            };
            for (key, item) in items {
                // dates: prefer the number (milliseconds) so the series code gets a real time
                let key = if kind == "date_histogram" || kind == "auto_date_histogram" { item.get("key").cloned().unwrap_or(key) } else { key };
                let mut p = prefix.clone();
                p.push((name.clone(), key));
                let mut here: Row = vec![("doc_count".into(), item.get("doc_count").cloned().unwrap_or(Value::Null))];
                let sub_defs = def.get("aggs").or_else(|| def.get("aggregations")).and_then(Value::as_object);
                let sub_has_buckets = sub_defs.is_some_and(|sd| sd.keys().any(|k| item.get(k).and_then(|v| v.get("buckets")).is_some()));
                if let (Some(sd), Some(io)) = (sub_defs, item.as_object()) {
                    if sub_has_buckets {
                        walk(sd, io, &p, rows, depth + 1);
                        continue;
                    }
                    for k in sd.keys() {
                        if let Some(m) = io.get(k) {
                            metric_cells(k, m, &mut here);
                        }
                    }
                }
                let mut row = p;
                row.extend(here);
                rows.push(row);
            }
        } else {
            metric_cells(name, r, &mut metrics);
        }
    }
    if !bucketed && !metrics.is_empty() {
        let mut row = prefix.clone();
        row.extend(metrics);
        rows.push(row);
    }
}

/// Build the frame of an `_search` answer for the request body `req` (needed to know which aggregation is a date histogram).
pub fn parse_search(req: &Value, resp: &Value, limit: usize) -> Result<Frame, DashError> {
    if let Some(e) = resp.get("error") {
        let r = e.get("reason").and_then(Value::as_str).or_else(|| e.as_str()).unwrap_or("the search failed");
        return Err(DashError::new(ErrKind::BadQuery, format!("Elasticsearch: {r}")));
    }
    let mut f = Frame::default();
    let defs = req.get("aggs").or_else(|| req.get("aggregations")).and_then(Value::as_object);
    let resp_aggs = resp.get("aggregations").and_then(Value::as_object);
    if let (Some(d), Some(a)) = (defs, resp_aggs) {
        let mut rows: Vec<Row> = vec![];
        walk(d, a, &vec![], &mut rows, 0);
        let mut names: Vec<String> = vec![];
        for r in &rows {
            for (k, _) in r {
                if !names.contains(k) {
                    names.push(k.clone());
                }
            }
        }
        let table: Vec<Vec<Value>> = rows.iter().map(|r| names.iter().map(|n| r.iter().find(|(k, _)| k == n).map_or(Value::Null, |(_, v)| v.clone())).collect()).collect();
        f.columns = names.iter().enumerate().map(|(i, n)| Column { name: n.clone(), ty: infer_type(n, table.iter().map(|r| &r[i])) }).collect();
        // columns that came from a date_histogram are times
        for (i, n) in names.iter().enumerate() {
            if find_kind(d, n).is_some_and(|k| k.contains("date_histogram")) {
                f.columns[i].ty = ColType::Time;
            }
        }
        f.rows = table;
        f.derive_series_for_aggs();
    }
    if f.columns.is_empty() {
        if let Some(hits) = resp.pointer("/hits/hits").and_then(Value::as_array) {
            let mut flat: Vec<BTreeMap<String, Value>> = vec![];
            let mut names: Vec<String> = vec![];
            for h in hits {
                let mut m = BTreeMap::new();
                flatten("", h.get("_source").unwrap_or(&Value::Null), &mut m, 0);
                if let Some(i) = h.get("_index") {
                    m.insert("_index".into(), i.clone());
                }
                if let Some(i) = h.get("_id") {
                    m.insert("_id".into(), i.clone());
                }
                for k in m.keys() {
                    if !names.contains(k) {
                        names.push(k.clone());
                    }
                }
                flat.push(m);
            }
            // time-like and short columns first, ids last
            names.sort_by_key(|n| (n.starts_with('_') as u8, !(n.contains("time") || n.starts_with('@')) as u8));
            let table: Vec<Vec<Value>> = flat.iter().map(|m| names.iter().map(|n| m.get(n).cloned().unwrap_or(Value::Null)).collect()).collect();
            f.columns = names.iter().enumerate().map(|(i, n)| Column { name: n.clone(), ty: infer_type(n, table.iter().map(|r| &r[i])) }).collect();
            f.rows = table;
            if let Some(t) = resp.pointer("/hits/total/value").and_then(Value::as_u64) {
                if t as usize > f.rows.len() {
                    f.meta.truncated = true;
                    f.meta.notices.push(format!("{} of {} matching documents shown", f.rows.len(), t));
                }
            }
        }
    }
    if f.columns.is_empty() {
        // size 0 without aggregations: the number of matching documents
        if let Some(t) = resp.pointer("/hits/total/value").and_then(Value::as_u64) {
            f.columns = vec![Column { name: "total".into(), ty: ColType::Int }];
            f.rows = vec![vec![Value::from(t)]];
        }
    }
    if resp.get("timed_out").and_then(Value::as_bool) == Some(true) {
        f.meta.truncated = true;
        f.meta.notices.push("Elasticsearch stopped at its time limit: the numbers may be incomplete".into());
    }
    if let Some(sh) = resp.pointer("/_shards/failed").and_then(Value::as_u64).filter(|n| *n > 0) {
        f.meta.notices.push(format!("{sh} shard(s) failed: the numbers may be incomplete"));
    }
    f.cap(limit, MAX_SERIES, MAX_POINTS);
    Ok(f)
}

fn find_kind<'a>(defs: &'a Map<String, Value>, name: &str) -> Option<&'a str> {
    for (k, d) in defs {
        if k == name {
            return agg_kind(d);
        }
        if let Some(sub) = d.get("aggs").or_else(|| d.get("aggregations")).and_then(Value::as_object) {
            if let Some(x) = find_kind(sub, name) {
                return Some(x);
            }
        }
    }
    None
}

impl Frame {
    /// After an aggregation table was built: with a time column, make series of the numeric columns split by the text columns.
    fn derive_series_for_aggs(&mut self) {
        let time = self.columns.iter().find(|c| c.ty == ColType::Time).map(|c| c.name.clone());
        if time.is_none() {
            return;
        }
        // the `doc_count` and metric columns are values; keys of terms aggregations are groups
        let vals: Vec<String> = self.columns.iter().filter(|c| matches!(c.ty, ColType::Int | ColType::Float) && c.name != time.clone().unwrap()).map(|c| c.name.clone()).collect();
        let grp: Vec<String> = self.columns.iter().filter(|c| c.ty == ColType::Str).map(|c| c.name.clone()).collect();
        let (v, g): (Vec<&str>, Vec<&str>) = (vals.iter().map(String::as_str).collect(), grp.iter().map(String::as_str).collect());
        self.series = self.series_from_table(&time.unwrap(), &v, &g);
    }
}

/// `{"a": {"b": 1}}` becomes `a.b: 1`; arrays are kept as JSON text.
fn flatten(prefix: &str, v: &Value, out: &mut BTreeMap<String, Value>, depth: usize) {
    match v {
        Value::Object(o) if depth < 5 => {
            for (k, x) in o {
                let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                flatten(&key, x, out, depth + 1);
            }
        }
        Value::Array(a) if a.iter().all(|x| !x.is_object() && !x.is_array()) && !a.is_empty() => {
            out.insert(prefix.to_string(), Value::from(a.iter().map(crate::dash::frame::cell_text).collect::<Vec<_>>().join(", ")));
        }
        Value::Null if prefix.is_empty() => {}
        other => {
            out.insert(prefix.to_string(), if other.is_object() || other.is_array() { Value::from(other.to_string()) } else { other.clone() });
        }
    }
}

fn mapping_fields(prefix: &str, props: &Value, out: &mut Vec<(String, String)>, depth: usize) {
    let Some(o) = props.as_object() else { return };
    for (k, v) in o {
        let name = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
        if let Some(t) = v.get("type").and_then(Value::as_str) {
            out.push((name.clone(), t.to_string()));
        }
        if depth < 6 {
            if let Some(p) = v.get("properties") {
                mapping_fields(&name, p, out, depth + 1);
            }
            if let Some(fs) = v.get("fields") {
                mapping_fields(&name, fs, out, depth + 1);
            }
        }
    }
}

impl DataSource for Elastic {
    fn id(&self) -> &str {
        &self.id
    }
    fn kind(&self) -> &str {
        "elasticsearch"
    }
    fn language(&self) -> Lang {
        Lang::EsDsl
    }

    fn test(&self, cancel: &Cancel) -> Result<TestInfo, DashError> {
        let v = self.call("GET", "/", &[], cancel)?;
        let ver = v.pointer("/version/number").and_then(Value::as_str).unwrap_or("?");
        let name = v.get("cluster_name").and_then(Value::as_str).unwrap_or("");
        let dist = if v.pointer("/version/distribution").and_then(Value::as_str) == Some("opensearch") { "OpenSearch" } else { "Elasticsearch" };
        let health = self.call("GET", "/_cluster/health", &[], cancel).ok().and_then(|h| h.get("status").and_then(Value::as_str).map(String::from));
        Ok(TestInfo { ok: true, detail: format!("{dist} {ver}{}{}", if name.is_empty() { String::new() } else { format!(" cluster {name}") }, health.map_or(String::new(), |h| format!(" ({h})"))) })
    }

    fn browse(&self, path: &[String], cancel: &Cancel) -> Result<Vec<Node>, DashError> {
        match path {
            [] => {
                let v = self.call("GET", "/_cat/indices?format=json&h=index,docs.count,store.size,health&s=index", &[], cancel)?;
                let mut out: Vec<Node> = v
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|i| {
                        let n = i.get("index")?.as_str()?;
                        let docs = i.get("docs.count").map(cell_text).unwrap_or_default();
                        let size = i.get("store.size").map(cell_text).unwrap_or_default();
                        Some(Node { name: n.to_string(), kind: NodeKind::Index, detail: format!("{docs} docs, {size}"), has_children: true })
                    })
                    .collect();
                out.truncate(MAX_ROWS);
                Ok(out)
            }
            [index] => {
                if !valid_index(index) {
                    return Err(DashError::new(ErrKind::BadQuery, format!("`{index}` is not a valid index name")));
                }
                let v = self.call("GET", &format!("/{index}/_mapping"), &[], cancel)?;
                let mut fields: Vec<(String, String)> = vec![];
                for (_, m) in v.as_object().into_iter().flatten().take(3) {
                    mapping_fields("", m.pointer("/mappings/properties").unwrap_or(&Value::Null), &mut fields, 0);
                }
                fields.sort();
                fields.dedup();
                Ok(fields.into_iter().map(|(n, t)| Node { name: n, kind: NodeKind::Field, detail: t, has_children: false }).collect())
            }
            _ => Ok(vec![]),
        }
    }

    fn query(&self, req: &QueryRequest, cancel: &Cancel) -> Result<Frame, DashError> {
        let started = std::time::Instant::now();
        let mut body: Value = serde_json::from_str(req.text.trim()).map_err(|e| {
            DashError::new(ErrKind::BadQuery, format!("the Elasticsearch query is not valid JSON: {e}")).fix("Write the search body, for example {\"query\":{\"match_all\":{}},\"size\":20}.")
        })?;
        let Some(obj) = body.as_object_mut() else {
            return Err(DashError::new(ErrKind::BadQuery, "the query must be a JSON object"));
        };
        let target = req.target.clone().filter(|t| !t.is_empty()).ok_or_else(|| DashError::new(ErrKind::BadQuery, "choose an index to search").fix("Pick an index or pattern (for example logs-*) in the panel."))?;
        if !valid_index(&target) {
            return Err(DashError::new(ErrKind::BadQuery, format!("`{target}` is not a valid index name")).fix("Use letters, digits and - _ . * , only."));
        }
        let timeout = req.timeout_s.unwrap_or(DEFAULT_TIMEOUT_S).clamp(1, MAX_TIMEOUT_S);
        obj.entry("timeout").or_insert(Value::from(format!("{timeout}s")));
        let has_aggs = obj.contains_key("aggs") || obj.contains_key("aggregations");
        let size = obj.get("size").and_then(Value::as_u64).unwrap_or(if has_aggs { 0 } else { req.limit() as u64 });
        obj.insert("size".into(), Value::from(size.min(req.limit() as u64).min(MAX_ROWS as u64)));
        obj.entry("track_total_hits").or_insert(Value::Bool(true));
        let text = serde_json::to_string(&body).unwrap_or_default();
        let resp = self.call("POST", &format!("/{target}/_search?ignore_unavailable=true&allow_no_indices=true"), text.as_bytes(), cancel)?;
        let mut f = parse_search(&body, &resp, req.limit())?;
        f.meta.took_ms = resp.get("took").and_then(Value::as_u64).unwrap_or_else(|| took(started));
        f.meta.executed = format!("POST /{target}/_search {}", req.text.trim().replace('\n', " "));
        Ok(f)
    }

    fn starter(&self, target: &str) -> String {
        let _ = target;
        "{\n  \"query\": {\"match_all\": {}},\n  \"size\": 20,\n  \"sort\": [{\"@timestamp\": \"desc\"}]\n}".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dash::connector::fake::*;
    use crate::wsexec::FakeRunner;
    use std::sync::Arc;

    fn src(up: &Arc<FakeUp>) -> Elastic {
        let d = deps(up, &[], Arc::new(FakeRunner::new(&[])));
        Elastic::new(&cfg("elasticsearch", "http://es:9200"), &d).unwrap()
    }

    fn q(text: &str) -> QueryRequest {
        let mut r = QueryRequest::new(text, range());
        r.target = Some("logs-*".into());
        r
    }

    const HITS: &str = r#"{"took":5,"timed_out":false,"_shards":{"total":2,"failed":0},"hits":{"total":{"value":3,"relation":"eq"},"hits":[
        {"_index":"logs-1","_id":"a","_source":{"@timestamp":"2026-10-15T12:00:00Z","level":"error","http":{"status":500},"tags":["x","y"]}},
        {"_index":"logs-1","_id":"b","_source":{"@timestamp":"2026-10-15T12:01:00Z","level":"info","http":{"status":200}}}]}}"#;

    #[test]
    fn hits_become_a_table_with_flattened_fields() {
        let up = FakeUp::new().on("_search", 200, HITS);
        let f = src(&up).query(&q(r#"{"query":{"match_all":{}}}"#), &Cancel::new()).unwrap();
        assert_eq!(f.columns[0].name, "@timestamp");
        assert_eq!(f.columns[0].ty, ColType::Time);
        assert!(f.columns.iter().any(|c| c.name == "http.status" && c.ty == ColType::Int));
        assert_eq!(f.rows.len(), 2);
        assert_eq!(f.cell(0, f.column_index("tags").unwrap()), "x, y");
        assert!(f.meta.truncated && f.meta.notices[0].contains("2 of 3"));
        let sent = up.bodies().remove(0);
        assert!(sent.contains("\"size\":1000") && sent.contains("\"timeout\":\"30s\""), "{sent}");
        assert!(up.urls()[0].starts_with("POST http://es:9200/logs-*/_search"));
    }

    const AGG: &str = r#"{"took":9,"timed_out":false,"hits":{"total":{"value":120,"relation":"eq"},"hits":[]},"aggregations":{
        "split":{"buckets":[
          {"key":"checkout","doc_count":70,"t":{"buckets":[{"key_as_string":"2026-10-15T12:00:00.000Z","key":1792065600000,"doc_count":30,"m":{"value":120.5}},{"key_as_string":"2026-10-15T12:01:00.000Z","key":1792065660000,"doc_count":40,"m":{"value":null}}]}},
          {"key":"cart","doc_count":50,"t":{"buckets":[{"key_as_string":"2026-10-15T12:00:00.000Z","key":1792065600000,"doc_count":50,"m":{"value":80}}]}}]}}}"#;

    #[test]
    fn a_terms_over_date_histogram_becomes_one_series_per_term() {
        let up = FakeUp::new().on("_search", 200, AGG);
        let text = date_histogram_query("@timestamp", "1m", "level:error", Some(("avg", "latency_ms")), Some(("service", 5)));
        let f = src(&up).query(&q(&text), &Cancel::new()).unwrap();
        assert_eq!(f.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["split", "t", "doc_count", "m"]);
        assert_eq!(f.columns[1].ty, ColType::Time);
        assert_eq!(f.rows.len(), 3);
        let names: Vec<&str> = f.series.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"checkout doc_count") && names.contains(&"cart m"), "{names:?}");
        let c = f.series.iter().find(|s| s.name == "checkout doc_count").unwrap();
        assert_eq!(c.points, vec![(1_792_065_600_000, 30.0), (1_792_065_660_000, 40.0)]);
        let sent = up.bodies().remove(0);
        assert!(sent.contains("\"size\":0") && sent.contains("$__from"), "{sent}");
    }

    #[test]
    fn terms_buckets_make_a_ranking_table() {
        let resp = r#"{"hits":{"total":{"value":10},"hits":[]},"aggregations":{"top":{"buckets":[{"key":"GET","doc_count":7},{"key":"POST","doc_count":3}]}}}"#;
        let up = FakeUp::new().on("_search", 200, resp);
        let f = src(&up).query(&q(&terms_query("http.method", 5, "")), &Cancel::new()).unwrap();
        assert_eq!(f.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["top", "doc_count"]);
        assert_eq!(f.rows[1][1], json!(3));
        assert!(f.series.is_empty());
    }

    #[test]
    fn a_metric_aggregation_without_buckets_is_one_row() {
        let resp = r#"{"hits":{"total":{"value":1},"hits":[]},"aggregations":{"p":{"values":{"50.0":10,"99.0":250}},"avg_l":{"value":12.5}}}"#;
        let up = FakeUp::new().on("_search", 200, resp);
        let f = src(&up).query(&q(r#"{"aggs":{"p":{"percentiles":{"field":"l"}},"avg_l":{"avg":{"field":"l"}}}}"#), &Cancel::new()).unwrap();
        assert_eq!(f.rows.len(), 1);
        assert!(f.columns.iter().any(|c| c.name == "p 99.0"));
    }

    #[test]
    fn server_errors_are_explained() {
        let body = r#"{"error":{"root_cause":[{"type":"parsing_exception","reason":"unknown query [matchh_all]"}],"type":"parsing_exception","reason":"unknown query [matchh_all]"},"status":400}"#;
        let up = FakeUp::new().on("_search", 400, body);
        let e = src(&up).query(&q(r#"{"query":{"matchh_all":{}}}"#), &Cancel::new()).unwrap_err();
        assert_eq!(e.kind, ErrKind::BadQuery);
        assert!(e.message.contains("unknown query [matchh_all]"), "{e}");
        let e = src(&FakeUp::new()).query(&q("{not json"), &Cancel::new()).unwrap_err();
        assert!(e.message.contains("not valid JSON"));
        let mut r = q("{}");
        r.target = None;
        assert!(src(&FakeUp::new()).query(&r, &Cancel::new()).unwrap_err().message.contains("index"));
        let mut r = q("{}");
        r.target = Some("../_bulk".into());
        assert!(src(&FakeUp::new()).query(&r, &Cancel::new()).is_err());
        let up = FakeUp::new().on("_search", 403, r#"{"error":{"reason":"action is unauthorized"}}"#);
        assert_eq!(src(&up).query(&q("{}"), &Cancel::new()).unwrap_err().kind, ErrKind::Auth);
    }

    #[test]
    fn browse_lists_indices_and_flattened_mapping() {
        let cat = r#"[{"index":"logs-1","docs.count":"1200","store.size":"3mb","health":"green"},{"index":".kibana","docs.count":"5","store.size":"1kb"}]"#;
        let map = r#"{"logs-1":{"mappings":{"properties":{"@timestamp":{"type":"date"},"http":{"properties":{"status":{"type":"integer"}}},"msg":{"type":"text","fields":{"raw":{"type":"keyword"}}}}}}}"#;
        let up = FakeUp::new().on("_cat/indices", 200, cat).on("/logs-1/_mapping", 200, map);
        let s = src(&up);
        let i = s.browse(&[], &Cancel::new()).unwrap();
        assert_eq!(i[0].detail, "1200 docs, 3mb");
        let f = s.browse(&["logs-1".into()], &Cancel::new()).unwrap();
        let got: Vec<String> = f.iter().map(|n| format!("{}:{}", n.name, n.detail)).collect();
        assert_eq!(got, vec!["@timestamp:date", "http.status:integer", "msg:text", "msg.raw:keyword"]);
        assert!(s.browse(&["x/../_all".into()], &Cancel::new()).is_err());
    }

    #[test]
    fn test_names_the_cluster_and_health() {
        let up = FakeUp::new().on("/_cluster/health", 200, r#"{"status":"yellow"}"#).on("http://es:9200/", 200, r#"{"cluster_name":"prod","version":{"number":"8.14.0"}}"#);
        assert_eq!(src(&up).test(&Cancel::new()).unwrap().detail, "Elasticsearch 8.14.0 cluster prod (yellow)");
    }

    #[test]
    fn odd_json_never_panics() {
        for body in ["", "null", "[]", "{}", r#"{"hits":5}"#, r#"{"aggregations":{"t":{"buckets":"x"}}}"#, r#"{"aggregations":[1]}"#, r#"{"hits":{"hits":[1,null,{"_source":[1,2]}]}}"#, r#"{"aggregations":{"t":{"buckets":[{"key":1,"t":{"buckets":[{"key":2}]}}]}}}"#] {
            let up = FakeUp::new().on("_search", 200, body);
            let _ = src(&up).query(&q(r#"{"aggs":{"t":{"date_histogram":{"field":"x","fixed_interval":"1m"},"aggs":{"t":{"terms":{"field":"y"}}}}}}"#), &Cancel::new());
        }
    }

    #[test]
    fn writes_cannot_be_reached() {
        let up = FakeUp::new();
        let s = src(&up);
        assert!(s.call("POST", "/_bulk", b"{}", &Cancel::new()).is_err());
        assert!(s.call("DELETE", "/logs", &[], &Cancel::new()).is_err());
        assert!(up.urls().is_empty(), "nothing was sent");
    }

    #[test]
    fn index_names() {
        assert!(valid_index("logs-*,metrics-2026.10") && valid_index("a:b"));
        for bad in ["", "_all/..", "a b", "a/b", "-x", "a?b", "a%2e"] {
            assert!(!valid_index(bad), "{bad}");
        }
    }
}
