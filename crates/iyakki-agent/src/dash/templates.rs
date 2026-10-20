//! Ready-made dashboards. Each one is built for a kind of source; choosing it binds every panel to the source picked.

use super::model::*;

pub struct Template {
    pub id: &'static str,
    pub title: &'static str,
    /// The kind of source it needs.
    pub kind: &'static str,
    pub about: &'static str,
    build: fn(&str) -> Dashboard,
}

impl Template {
    pub fn instantiate(&self, source: &str, dashboard_id: &str) -> Dashboard {
        let mut d = (self.build)(source);
        d.id = dashboard_id.into();
        d.sanitize();
        d
    }
}

pub const GREEN: [u8; 3] = [80, 220, 140];
pub const AMBER: [u8; 3] = [255, 196, 94];
pub const RED: [u8; 3] = [255, 100, 110];

pub fn all() -> Vec<Template> {
    vec![
        Template { id: "node-exporter", title: "Prometheus: node CPU, memory, disk", kind: "prometheus", about: "Hosts monitored by node_exporter: CPU, memory, disk, network, load.", build: node_exporter },
        Template { id: "kube-state", title: "Kubernetes pods (kube-state-metrics)", kind: "prometheus", about: "Pods by phase, restarts, container memory and CPU per namespace.", build: kube_state },
        Template { id: "elasticsearch-health", title: "Elasticsearch: index doc counts and cluster health", kind: "elasticsearch", about: "Documents per index and a rate over time.", build: elasticsearch },
        Template { id: "clickhouse-queries", title: "ClickHouse: system.query_log top queries", kind: "clickhouse", about: "Slowest and most frequent queries of the last day, and queries per minute.", build: clickhouse },
        Template { id: "duckdb-explore", title: "DuckDB: explore a CSV/Parquet file", kind: "duckdb", about: "Row count, first rows and the columns of the first data file of the source.", build: duckdb },
    ]
}

pub fn find(id: &str) -> Option<Template> {
    all().into_iter().find(|t| t.id == id)
}

fn node_exporter(s: &str) -> Dashboard {
    let mut d = Dashboard::new("node-exporter", "Node CPU, memory, disk");
    d.description = "node_exporter metrics. Pick the host with the instance variable.".into();
    d.refresh_s = 30;
    d.variables = vec![Variable { name: "instance".into(), label: "Host".into(), kind: VarKind::Query { source: s.into(), query: "node_uname_info".into(), target: None }, current: vec![], multi: true, include_all: true, mapping: Default::default() }];
    let th = [(70.0, AMBER), (90.0, RED)];
    let p = |id: &str, t: &str, v: Visual, q: &str, g: Grid| Panel::new(id, t, s, v, q, g);
    d.panels = vec![
        p("cpu", "CPU used", Visual::Stat, "100 - avg(rate(node_cpu_seconds_total{mode=\"idle\",instance=~\"$instance\"}[5m])) * 100", Grid::new(0, 0, 6, 5)).unit(Unit::Percent).thresholds(&th),
        p("mem", "Memory used", Visual::Gauge, "100 * (1 - sum(node_memory_MemAvailable_bytes{instance=~\"$instance\"}) / sum(node_memory_MemTotal_bytes{instance=~\"$instance\"}))", Grid::new(6, 0, 6, 5)).unit(Unit::Percent).thresholds(&th),
        p("disk", "Root disk used", Visual::Gauge, "100 * (1 - sum(node_filesystem_avail_bytes{mountpoint=\"/\",fstype!~\"tmpfs|overlay\",instance=~\"$instance\"}) / sum(node_filesystem_size_bytes{mountpoint=\"/\",fstype!~\"tmpfs|overlay\",instance=~\"$instance\"}))", Grid::new(12, 0, 6, 5)).unit(Unit::Percent).thresholds(&th),
        p("load", "Load (1 min)", Visual::Stat, "avg(node_load1{instance=~\"$instance\"})", Grid::new(18, 0, 6, 5)).unit(Unit::Number),
        p("cpu-ts", "CPU by host", Visual::Area, "100 - avg by (instance) (rate(node_cpu_seconds_total{mode=\"idle\",instance=~\"$instance\"}[$__interval])) * 100", Grid::new(0, 5, 12, 8)).unit(Unit::Percent).legend_format("{{instance}}"),
        p("mem-ts", "Memory available", Visual::Line, "node_memory_MemAvailable_bytes{instance=~\"$instance\"}", Grid::new(12, 5, 12, 8)).unit(Unit::Bytes).legend_format("{{instance}}"),
        p("net", "Network in", Visual::Line, "sum by (instance) (rate(node_network_receive_bytes_total{device!~\"lo|veth.*|docker.*\",instance=~\"$instance\"}[$__interval]))", Grid::new(0, 13, 12, 8)).unit(Unit::BytesPerSec).legend_format("{{instance}}"),
        p("disk-io", "Disk read + write", Visual::Line, "sum by (instance) (rate(node_disk_read_bytes_total{instance=~\"$instance\"}[$__interval]) + rate(node_disk_written_bytes_total{instance=~\"$instance\"}[$__interval]))", Grid::new(12, 13, 12, 8)).unit(Unit::BytesPerSec).legend_format("{{instance}}"),
    ];
    d
}

fn kube_state(s: &str) -> Dashboard {
    let mut d = Dashboard::new("kube-state", "Kubernetes pods");
    d.description = "kube-state-metrics and cAdvisor series.".into();
    d.refresh_s = 30;
    d.variables = vec![Variable { name: "namespace".into(), label: "Namespace".into(), kind: VarKind::Query { source: s.into(), query: "count by (namespace) (kube_pod_info)".into(), target: None }, current: vec![], multi: true, include_all: true, mapping: Default::default() }];
    let p = |id: &str, t: &str, v: Visual, q: &str, g: Grid| Panel::new(id, t, s, v, q, g);
    d.panels = vec![
        p("running", "Running pods", Visual::Stat, "sum(kube_pod_status_phase{phase=\"Running\",namespace=~\"$namespace\"})", Grid::new(0, 0, 6, 5)).unit(Unit::Number),
        p("pending", "Pending pods", Visual::Stat, "sum(kube_pod_status_phase{phase=\"Pending\",namespace=~\"$namespace\"})", Grid::new(6, 0, 6, 5)).unit(Unit::Number).thresholds(&[(1.0, AMBER), (10.0, RED)]),
        p("failed", "Failed pods", Visual::Stat, "sum(kube_pod_status_phase{phase=\"Failed\",namespace=~\"$namespace\"})", Grid::new(12, 0, 6, 5)).unit(Unit::Number).thresholds(&[(1.0, RED)]),
        p("restarts", "Restarts (last hour)", Visual::Stat, "sum(increase(kube_pod_container_status_restarts_total{namespace=~\"$namespace\"}[1h]))", Grid::new(18, 0, 6, 5)).unit(Unit::Number).thresholds(&[(1.0, AMBER), (10.0, RED)]),
        p("phase", "Pods by phase", Visual::Donut, "sum by (phase) (kube_pod_status_phase{namespace=~\"$namespace\"} > 0)", Grid::new(0, 5, 8, 8)).legend_format("{{phase}}"),
        p("mem", "Memory by namespace", Visual::Area, "sum by (namespace) (container_memory_working_set_bytes{container!=\"\",namespace=~\"$namespace\"})", Grid::new(8, 5, 16, 8)).unit(Unit::Bytes).legend_format("{{namespace}}"),
        p("cpu", "CPU by namespace", Visual::Line, "sum by (namespace) (rate(container_cpu_usage_seconds_total{container!=\"\",namespace=~\"$namespace\"}[$__interval]))", Grid::new(0, 13, 12, 8)).legend_format("{{namespace}}"),
        p("top-restarts", "Most restarting pods", Visual::Table, "topk(10, sum by (namespace, pod) (increase(kube_pod_container_status_restarts_total{namespace=~\"$namespace\"}[1h])))", Grid::new(12, 13, 12, 8)),
    ];
    d
}

fn elasticsearch(s: &str) -> Dashboard {
    let mut d = Dashboard::new("elasticsearch-health", "Elasticsearch indices");
    d.refresh_s = 60;
    let p = |id: &str, t: &str, v: Visual, q: &str, g: Grid| Panel::new(id, t, s, v, q, g);
    d.panels = vec![
        p("docs", "Documents in all indices", Visual::Stat, "{\"size\":0,\"track_total_hits\":true}", Grid::new(0, 0, 6, 5)).target("*,-.*").unit(Unit::Number),
        p("per-index", "Documents per index", Visual::Bar, "{\"size\":0,\"aggs\":{\"idx\":{\"terms\":{\"field\":\"_index\",\"size\":15}}}}", Grid::new(6, 0, 18, 8)).target("*,-.*"),
        p("rate", "Documents over time", Visual::Area, "{\"size\":0,\"query\":{\"range\":{\"@timestamp\":{\"gte\":\"$__from\",\"lte\":\"$__to\",\"format\":\"epoch_millis\"}}},\"aggs\":{\"t\":{\"date_histogram\":{\"field\":\"@timestamp\",\"fixed_interval\":\"$__interval\",\"min_doc_count\":0}}}}", Grid::new(0, 8, 24, 8)).target("*,-.*"),
    ];
    d.panels[0].title = "Documents in all indices".into();
    d
}

fn clickhouse(s: &str) -> Dashboard {
    let mut d = Dashboard::new("clickhouse-queries", "ClickHouse: top queries");
    d.refresh_s = 60;
    d.time = TimeSpec { from: "now-24h".into(), to: "now".into() };
    let p = |id: &str, t: &str, v: Visual, q: &str, g: Grid| Panel::new(id, t, s, v, q, g);
    d.panels = vec![
        p("slow", "Slowest queries", Visual::Table, "SELECT normalizeQuery(query) AS query, count() AS calls, round(avg(query_duration_ms)) AS avg_ms, max(query_duration_ms) AS max_ms, formatReadableSize(sum(read_bytes)) AS read\nFROM system.query_log\nWHERE type = 'QueryFinish' AND event_time >= toDateTime($__from_s) AND event_time <= toDateTime($__to_s)\nGROUP BY query ORDER BY avg_ms DESC LIMIT 20", Grid::new(0, 0, 24, 9)),
        p("frequent", "Most frequent queries", Visual::Bar, "SELECT substring(normalizeQuery(query), 1, 40) AS query, count() AS calls\nFROM system.query_log\nWHERE type = 'QueryFinish' AND event_time >= toDateTime($__from_s) AND event_time <= toDateTime($__to_s)\nGROUP BY query ORDER BY calls DESC LIMIT 10", Grid::new(0, 9, 12, 8)),
        p("per-min", "Queries per interval", Visual::Area, "SELECT toStartOfInterval(event_time, INTERVAL $__interval_s second) AS time, count() AS queries\nFROM system.query_log\nWHERE type = 'QueryFinish' AND event_time >= toDateTime($__from_s) AND event_time <= toDateTime($__to_s)\nGROUP BY time ORDER BY time", Grid::new(12, 9, 12, 8)),
    ];
    d
}

fn duckdb(s: &str) -> Dashboard {
    let mut d = Dashboard::new("duckdb-explore", "DuckDB: explore a file");
    d.description = "Edit the queries to name your own file: SELECT * FROM read_csv_auto('/home/you/data.csv'). The source's first data file is used by the starter panels through the `file` variable.".into();
    d.variables = vec![Variable { name: "file".into(), label: "File".into(), kind: VarKind::Custom { options: vec!["/home/you/data.csv".into()] }, current: vec![], multi: false, include_all: false, mapping: Default::default() }];
    let p = |id: &str, t: &str, v: Visual, q: &str, g: Grid| Panel::new(id, t, s, v, q, g);
    d.panels = vec![
        p("rows", "Rows", Visual::Stat, "SELECT count(*) AS rows FROM read_csv_auto('$file')", Grid::new(0, 0, 6, 5)).unit(Unit::Number),
        p("columns", "Columns", Visual::Table, "DESCRIBE SELECT * FROM read_csv_auto('$file')", Grid::new(6, 0, 18, 8)),
        p("head", "First rows", Visual::Table, "SELECT * FROM read_csv_auto('$file') LIMIT 100", Grid::new(0, 8, 24, 10)),
    ];
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_templates_build_parse_and_do_not_overlap() {
        for t in all() {
            let d = t.instantiate("src1", "tpl");
            assert!(!d.panels.is_empty(), "{}", t.id);
            assert!(d.panels.iter().all(|p| p.source == "src1"), "{}", t.id);
            let j = serde_json::to_string(&d).unwrap();
            let back: Dashboard = serde_json::from_str(&j).unwrap();
            assert_eq!(d, back, "{}", t.id);
            for (i, a) in d.panels.iter().enumerate() {
                for b in d.panels.iter().skip(i + 1) {
                    assert!(!a.grid.overlaps(&b.grid), "{}: {} / {}", t.id, a.id, b.id);
                }
            }
            for p in &d.panels {
                assert!(p.visual.needs_query() == !p.query.text.is_empty(), "{}", p.id);
            }
            assert!(super::super::registry::kind(t.kind).is_some(), "{}", t.id);
        }
        assert_eq!(all().len(), 5);
    }

    #[test]
    fn promql_and_sql_templates_pass_the_classifier() {
        use crate::dash::readonly::{sql_read_only, Dialect};
        for t in all().iter().filter(|t| t.kind == "clickhouse") {
            for p in t.instantiate("s", "x").panels {
                sql_read_only(&p.query.text, Dialect::ClickHouse).unwrap_or_else(|e| panic!("{}: {e}", p.id));
            }
        }
        for t in all().iter().filter(|t| t.kind == "duckdb") {
            for p in t.instantiate("s", "x").panels {
                sql_read_only(&p.query.text.replace("$file", "/home/you/data.csv"), Dialect::DuckDb).unwrap_or_else(|e| panic!("{}: {e}", p.id));
            }
        }
    }
}
