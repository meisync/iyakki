//! Running a panel with several queries: each query goes to its own source on its own thread (with its own timeout), identical
//! questions (same source, text, target, range and step) share one fetch through a cache, a failing query leaves the others'
//! data on screen with an error for that query, and the expression stage combines the frames.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use super::connector::*;
use super::engine::{interpolate, RunCtx};
use super::expr;
use super::frame::*;
use super::model::*;

type Slot = Arc<OnceLock<Result<Arc<Frame>, DashError>>>;

/// Answers shared between panels (and between the queries of one panel). Cleared on a forced refresh.
#[derive(Default)]
pub struct FetchCache {
    map: Mutex<BTreeMap<String, Slot>>,
    /// How many real requests were made (tests and the status line).
    pub fetches: std::sync::atomic::AtomicUsize,
}

impl FetchCache {
    pub fn new() -> Arc<FetchCache> {
        Arc::new(FetchCache::default())
    }
    pub fn clear(&self) {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
    pub fn len(&self) -> usize {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// The answer for `key`: made by `f` once, however many callers ask at the same time. A cancelled or failed answer is not kept.
    fn get(&self, key: &str, f: impl FnOnce() -> Result<Frame, DashError>) -> Result<Arc<Frame>, DashError> {
        let slot = {
            let mut m = self.map.lock().unwrap_or_else(|e| e.into_inner());
            if m.len() > 400 {
                m.clear();
            }
            m.entry(key.to_string()).or_default().clone()
        };
        let r = slot.get_or_init(|| {
            self.fetches.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            f().map(Arc::new)
        });
        if r.is_err() {
            self.map.lock().unwrap_or_else(|e| e.into_inner()).remove(key);
        }
        r.clone()
    }
}

/// One query's failure.
#[derive(Clone, Debug, PartialEq)]
pub struct QueryError {
    pub letter: char,
    pub source: String,
    pub error: DashError,
}

#[derive(Debug, Default)]
pub struct Outcome {
    pub frame: Frame,
    pub errors: Vec<QueryError>,
    /// Lines of the expression stage that failed.
    pub expr_errors: Vec<String>,
}

impl Outcome {
    /// Nothing at all could be shown: every query failed.
    pub fn failed(&self) -> bool {
        self.frame.is_empty() && !self.errors.is_empty()
    }
    /// One text for every problem (shown in the panel and read by the assistant).
    pub fn problems(&self) -> Vec<String> {
        self.errors.iter().map(|e| format!("{} ({}): {}", e.letter, e.source, e.error)).chain(self.expr_errors.iter().map(|e| format!("expression {e}"))).collect()
    }
}

/// The source of a query: found by id (or name) with the account's policy applied.
pub type Resolve<'a> = &'a (dyn Fn(&str) -> Result<(SourceConfig, Box<dyn DataSource>), DashError> + Sync);

fn key_of(source: &str, req: &QueryRequest) -> String {
    format!("{source}\u{1}{}\u{1}{:?}\u{1}{}\u{1}{}\u{1}{:?}\u{1}{}", req.text, req.target, req.range.from_ms, req.range.to_ms, req.step_ms, req.limit)
}

fn one(letter: char, def: &QueryDef, visual: Visual, ctx: &RunCtx, resolve: Resolve, cache: &FetchCache, cancel: &Cancel) -> Result<Arc<Frame>, QueryError> {
    let fail = |e: DashError| QueryError { letter, source: def.source.clone(), error: e };
    if def.query.text.trim().is_empty() {
        return Err(fail(DashError::new(ErrKind::BadQuery, "this query is empty").fix("Write a query, or remove it.")));
    }
    if def.source.is_empty() {
        return Err(fail(DashError::config("this query has no source").fix("Pick a source for it, or set a default source for the dashboard.")));
    }
    let (cfg, src) = resolve(&def.source).map_err(fail)?;
    let timeseries_like = matches!(visual, Visual::Line | Visual::Area | Visual::Bar | Visual::StackedBar | Visual::Heatmap);
    let step = ctx.step_ms(def.query.step_s);
    let vars = ctx.vars_for(&cfg.id);
    let mut c2 = ctx.clone();
    c2.vars = vars;
    let default_target = Some(cfg.value("default_index")).filter(|s| !s.is_empty());
    let req = QueryRequest {
        text: interpolate(&def.query.text, &c2, step),
        target: def.query.target.clone().filter(|t| !t.is_empty()).map(|t| interpolate(&t, &c2, step)).or(default_target.map(String::from)),
        range: ctx.range,
        step_ms: timeseries_like.then_some(step),
        limit: if visual == Visual::Table { 2000 } else { 1000 },
        timeout_s: def.query.timeout_s.map(u64::from).or(ctx.timeout_s),
    };
    let key = key_of(&cfg.id, &req);
    cache.get(&key, || src.query(&req, cancel)).map_err(fail)
}

/// Run every query of `panel` (in parallel), then the expressions, and give back one frame plus the errors.
pub fn run(panel: &Panel, default_source: &str, ctx: &RunCtx, resolve: Resolve, cache: &FetchCache, cancel: &Cancel, opts: &expr::Opts) -> Outcome {
    let mut out = Outcome::default();
    if !panel.visual.needs_query() {
        return out;
    }
    let defs = panel.query_defs(default_source);
    // empty extra queries are ignored; A may be empty only when there are others
    let defs: Vec<(char, QueryDef)> = defs.into_iter().enumerate().filter(|(i, d)| *i == 0 || !d.1.query.text.trim().is_empty()).map(|(_, d)| d).collect();
    let results: Vec<Result<Arc<Frame>, QueryError>> = if defs.len() == 1 {
        vec![one(defs[0].0, &defs[0].1, panel.visual, ctx, resolve, cache, cancel)]
    } else {
        std::thread::scope(|sc| {
            let hs: Vec<_> = defs.iter().map(|(l, d)| sc.spawn(move || one(*l, d, panel.visual, ctx, resolve, cache, cancel))).collect();
            hs.into_iter().zip(&defs).map(|(h, (l, d))| h.join().unwrap_or_else(|_| Err(QueryError { letter: *l, source: d.source.clone(), error: DashError::new(ErrKind::Server, "the query stopped unexpectedly") }))).collect()
        })
    };
    let multi = defs.len() > 1;
    let mut frames: BTreeMap<char, Frame> = BTreeMap::new();
    let mut shown: Vec<(char, Frame)> = vec![];
    let mut right_unit: Option<Unit> = None;
    for ((letter, def), r) in defs.iter().zip(results) {
        match r {
            Ok(f) => {
                let mut f = (*f).clone();
                let alias = if !def.query.alias.is_empty() { def.query.alias.clone() } else if *letter == 'A' { panel.options.legend_format.clone() } else { String::new() };
                for s in &mut f.series {
                    if !alias.trim().is_empty() {
                        s.name = series_label(&alias, &s.name, &s.labels);
                    } else if multi {
                        s.name = format!("{letter}: {}", s.name);
                    }
                    s.axis = u8::from(def.query.axis == Axis::Right);
                }
                if def.query.axis == Axis::Right {
                    right_unit = def.query.unit.or(right_unit);
                }
                frames.insert(*letter, f.clone());
                if !def.query.hidden {
                    shown.push((*letter, f));
                }
            }
            Err(e) => out.errors.push(e),
        }
    }
    // the stage after the queries
    let (eo, lines) = expr::split_directives(&panel.expressions, *opts);
    let x = if lines.is_empty() { expr::Output::default() } else { expr::run(&lines, &frames, &eo) };
    out.expr_errors = x.errors;
    let mut combined = Frame::default();
    for (_, f) in &shown {
        combined.series.extend(f.series.iter().cloned());
        combined.meta.truncated |= f.meta.truncated;
        combined.meta.took_ms = combined.meta.took_ms.max(f.meta.took_ms);
        combined.meta.notices.extend(f.meta.notices.iter().cloned());
        if combined.columns.is_empty() && !f.columns.is_empty() {
            combined.columns = f.columns.clone();
            combined.rows = f.rows.clone();
        }
        if !f.meta.executed.is_empty() {
            if !combined.meta.executed.is_empty() {
                combined.meta.executed.push_str("\n");
            }
            combined.meta.executed.push_str(&f.meta.executed);
        }
    }
    combined.series.extend(x.series);
    if let Some(t) = x.table {
        combined.columns = t.columns;
        combined.rows = t.rows;
        combined.series.clear();
    }
    combined.meta.right_unit = right_unit.map(|u| format!("{u:?}").to_lowercase());
    if combined.series.len() > MAX_SERIES {
        combined.cap(MAX_ROWS, MAX_SERIES, MAX_POINTS);
    }
    out.frame = combined;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dash::connector::fake::*;
    use crate::wsexec::FakeRunner;
    use std::sync::atomic::Ordering;

    const PROM: &str = r#"{"status":"success","data":{"resultType":"matrix","result":[{"metric":{"host":"web1"},"values":[[1792065600,"50"],[1792065660,"60"]]}]}}"#;
    const ES: &str = r#"{"hits":{"total":{"value":2},"hits":[]},"aggregations":{"t":{"buckets":[{"key":1792065605000,"key_as_string":"x","doc_count":5},{"key":1792065665000,"key_as_string":"y","doc_count":10}]}}}"#;

    struct World {
        up: std::sync::Arc<FakeUp>,
        deps: Deps,
        sources: Vec<SourceConfig>,
    }

    fn world() -> World {
        let up = FakeUp::new().on("query_range", 200, PROM).on("_search", 200, ES);
        let deps = deps(&up, &[], std::sync::Arc::new(FakeRunner::new(&[])));
        let mut p = cfg("prometheus", "http://prom:9090");
        p.id = "prom".into();
        let mut e = cfg("elasticsearch", "http://es:9200");
        e.id = "es".into();
        World { up, deps, sources: vec![p, e] }
    }

    fn resolver(w: &World) -> impl Fn(&str) -> Result<(SourceConfig, Box<dyn DataSource>), DashError> + Sync + '_ {
        move |id: &str| {
            let c = w.sources.iter().find(|s| s.id == id).ok_or_else(|| DashError::config(format!("there is no source `{id}`")))?;
            Ok((c.clone(), crate::dash::registry::build(c, &w.deps)?))
        }
    }

    fn ctx() -> RunCtx {
        RunCtx::new(range())
    }

    fn mixed() -> Panel {
        let mut p = Panel::new("p1", "Mixed", "prom", Visual::Line, "cpu", Grid::new(0, 0, 12, 8));
        p.queries.push(QueryDef { source: "es".into(), query: PanelQuery { text: r#"{"aggs":{"t":{"date_histogram":{"field":"@timestamp","fixed_interval":"1m"}}}}"#.into(), target: Some("logs-*".into()), axis: Axis::Right, unit: Some(Unit::Number), alias: "errors".into(), ..Default::default() } });
        p
    }

    #[test]
    fn two_sources_in_one_panel_with_an_alias_and_a_right_axis() {
        let w = world();
        let r = resolver(&w);
        let o = run(&mixed(), "", &ctx(), &r, &FetchCache::new(), &Cancel::new(), &expr::Opts::default());
        assert!(o.errors.is_empty(), "{:?}", o.errors);
        let names: Vec<&str> = o.frame.series.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["A: {host=\"web1\"}", "errors"], "{names:?}");
        assert_eq!((o.frame.series[0].axis, o.frame.series[1].axis), (0, 1));
        assert_eq!(o.frame.meta.right_unit.as_deref(), Some("number"));
    }

    #[test]
    fn a_failing_source_leaves_the_others_data_and_names_the_failure() {
        let mut w = world();
        // the Elasticsearch server is down
        w.sources[1].values.insert("url".into(), "http://es-down:9200".into());
        let up2 = FakeUp::new().on("query_range", 200, PROM);
        *up2.fail.lock().unwrap() = None;
        let mut deps2 = deps(&up2, &[], std::sync::Arc::new(FakeRunner::new(&[])));
        deps2.transport = std::sync::Arc::new(Split { prom: up2.clone(), es: { let u = FakeUp::new(); *u.fail.lock().unwrap() = Some("Connection Failed: Connection refused".into()); u } });
        w.deps = deps2;
        let r = resolver(&w);
        let o = run(&mixed(), "", &ctx(), &r, &FetchCache::new(), &Cancel::new(), &expr::Opts::default());
        assert_eq!(o.frame.series.len(), 1, "the Prometheus data is still there");
        assert_eq!(o.errors.len(), 1);
        assert_eq!(o.errors[0].letter, 'B');
        assert!(o.errors[0].error.to_string().contains("connection refused"), "{}", o.errors[0].error);
        assert!(!o.failed());
        assert!(o.problems()[0].starts_with("B (es):"));
    }

    struct Split {
        prom: std::sync::Arc<FakeUp>,
        es: std::sync::Arc<FakeUp>,
    }
    impl Transport for Split {
        fn upstream(&self, o: &NetOptions) -> Result<std::sync::Arc<dyn iyakki_proxy::Upstream>, String> {
            let _ = o;
            Ok(std::sync::Arc::new(Routing(self.prom.clone(), self.es.clone())))
        }
    }
    struct Routing(std::sync::Arc<FakeUp>, std::sync::Arc<FakeUp>);
    impl iyakki_proxy::Upstream for Routing {
        fn request(&self, m: &str, u: &str, h: &[(String, String)], b: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            if u.contains("es-down") { self.1.request(m, u, h, b) } else { self.0.request(m, u, h, b) }
        }
    }

    #[test]
    fn all_queries_failing_is_a_failed_panel() {
        let w = world();
        let r = resolver(&w);
        let mut p = mixed();
        p.source = "nope".into();
        p.queries[0].source = "nope2".into();
        let o = run(&p, "", &ctx(), &r, &FetchCache::new(), &Cancel::new(), &expr::Opts::default());
        assert!(o.failed() && o.errors.len() == 2);
        assert!(o.errors[0].error.to_string().contains("there is no source"));
    }

    #[test]
    fn identical_queries_share_one_fetch_across_panels_and_queries() {
        let w = world();
        let r = resolver(&w);
        let cache = FetchCache::new();
        let p1 = Panel::new("p1", "One", "prom", Visual::Line, "cpu", Grid::new(0, 0, 12, 8));
        let mut p2 = Panel::new("p2", "Two", "prom", Visual::Area, "cpu", Grid::new(12, 0, 12, 8));
        p2.queries.push(QueryDef { source: "prom".into(), query: PanelQuery { text: "cpu".into(), ..Default::default() } });
        for p in [&p1, &p2] {
            run(p, "", &ctx(), &r, &cache, &Cancel::new(), &expr::Opts::default());
        }
        assert_eq!(cache.fetches.load(Ordering::SeqCst), 1, "{:?}", w.up.urls());
        assert_eq!(w.up.urls().len(), 1);
        // a different range, query or source is a different fetch
        let mut c2 = ctx();
        c2.range.from_ms -= 1000;
        run(&p1, "", &c2, &r, &cache, &Cancel::new(), &expr::Opts::default());
        assert_eq!(cache.fetches.load(Ordering::SeqCst), 2);
        cache.clear();
        run(&p1, "", &ctx(), &r, &cache, &Cancel::new(), &expr::Opts::default());
        assert_eq!(cache.fetches.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn parallel_queries_share_a_fetch_even_when_asked_at_once() {
        let w = world();
        let r = resolver(&w);
        let cache = FetchCache::new();
        let mut p = Panel::new("p1", "Same", "prom", Visual::Line, "cpu", Grid::new(0, 0, 12, 8));
        for _ in 0..4 {
            p.queries.push(QueryDef { source: "prom".into(), query: PanelQuery { text: "cpu".into(), ..Default::default() } });
        }
        let o = run(&p, "", &ctx(), &r, &cache, &Cancel::new(), &expr::Opts::default());
        assert_eq!(o.frame.series.len(), 5);
        assert_eq!(cache.fetches.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_dashboard_default_source_and_expressions_across_sources() {
        let w = world();
        let r = resolver(&w);
        let mut p = mixed();
        p.source = String::new(); // A uses the default
        p.expressions = vec!["C = B / A * 100".into()];
        let o = run(&p, "prom", &ctx(), &r, &FetchCache::new(), &Cancel::new(), &expr::Opts { align: expr::Align::Nearest, tolerance_ms: 30_000 });
        assert!(o.errors.is_empty() && o.expr_errors.is_empty(), "{:?} {:?}", o.errors, o.expr_errors);
        let c = o.frame.series.iter().find(|s| s.name == "C").expect("the expression result is drawn");
        // errors (5 at +5 s, 10 at +65 s) over cpu (50 at 0, 60 at +60 s), aligned to the nearest within 30 s
        assert_eq!(c.points, vec![(1_792_065_605_000, 10.0), (1_792_065_665_000, 10.0 / 60.0 * 100.0)]);
        assert!(o.frame.series.len() == 3);
    }

    #[test]
    fn hidden_queries_feed_expressions_but_are_not_drawn_and_bad_lines_do_not_blank_the_panel() {
        let w = world();
        let r = resolver(&w);
        let mut p = mixed();
        p.query.hidden = true;
        p.expressions = vec!["C = A * 2".into(), "D = nonsense +".into()];
        let o = run(&p, "", &ctx(), &r, &FetchCache::new(), &Cancel::new(), &expr::Opts::default());
        let names: Vec<&str> = o.frame.series.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["errors", "C"], "{names:?}");
        assert_eq!(o.expr_errors.len(), 1);
        assert!(o.problems().iter().any(|p| p.starts_with("expression D:")));
    }

    #[test]
    fn variables_are_mapped_per_source_and_a_panel_without_text_is_skipped() {
        let w = world();
        let r = resolver(&w);
        let mut c = ctx();
        c.vars.insert("host".into(), vec!["web1".into()]);
        c.var_maps.insert("host".into(), [("es".to_string(), "{}.prod.example.com".to_string())].into());
        let mut p = Panel::new("p1", "V", "prom", Visual::Line, "cpu{host=\"$host\"}", Grid::new(0, 0, 12, 8));
        p.queries.push(QueryDef { source: "es".into(), query: PanelQuery { text: r#"{"query":{"term":{"host":"$host"}},"aggs":{"t":{"date_histogram":{"field":"@timestamp","fixed_interval":"1m"}}}}"#.into(), target: Some("logs-*".into()), ..Default::default() } });
        p.queries.push(QueryDef::default());
        run(&p, "", &c, &r, &FetchCache::new(), &Cancel::new(), &expr::Opts::default());
        let bodies = w.up.bodies().join("\n");
        assert!(bodies.contains("web1.prod.example.com"), "Elasticsearch got the mapped value: {bodies}");
        let urls = w.up.urls().join("\n");
        assert!(urls.contains("host%3D%22web1%22"), "Prometheus got the plain value: {urls}");
    }

    #[test]
    fn a_cancelled_run_keeps_nothing() {
        let w = world();
        let r = resolver(&w);
        let cache = FetchCache::new();
        let c = Cancel::new();
        c.cancel();
        let o = run(&mixed(), "", &ctx(), &r, &cache, &c, &expr::Opts::default());
        assert!(o.failed());
        assert!(o.errors.iter().all(|e| e.error.kind == ErrKind::Cancelled));
        assert!(cache.is_empty(), "failed answers are not cached");
    }
}
