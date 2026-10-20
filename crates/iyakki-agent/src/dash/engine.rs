//! Running a panel: fill in the dashboard's variables and time macros, build the request, ask the source, shape the answer for
//! the panel's visual. No threads here: callers (the screen's worker, the assistant's tools) own those.

use std::collections::BTreeMap;

use super::connector::*;
use super::frame::*;
use super::model::*;

/// What a run knows: the time range, the width in pixels the chart will have (picks the step), and variable values.
#[derive(Clone, Debug)]
pub struct RunCtx {
    pub range: TimeRange,
    pub width_px: usize,
    pub vars: BTreeMap<String, Vec<String>>,
    /// Per variable, per source id: how the value is written for that source (`{}` is the value).
    pub var_maps: BTreeMap<String, BTreeMap<String, String>>,
    pub timeout_s: Option<u64>,
}

impl RunCtx {
    pub fn new(range: TimeRange) -> RunCtx {
        RunCtx { range, width_px: 600, vars: BTreeMap::new(), var_maps: BTreeMap::new(), timeout_s: None }
    }

    /// The variables as one source sees them (its mapping applied to every value).
    pub fn vars_for(&self, source: &str) -> BTreeMap<String, Vec<String>> {
        self.vars
            .iter()
            .map(|(k, vals)| {
                let f = self.var_maps.get(k).and_then(|m| m.get(source));
                (k.clone(), vals.iter().map(|v| f.map_or_else(|| v.clone(), |f| f.replace("{}", v))).collect())
            })
            .collect()
    }

    /// Resolution of a series: about one point per two pixels, never finer than the source allows.
    pub fn step_ms(&self, panel_step_s: Option<u32>) -> i64 {
        match panel_step_s {
            Some(s) if s > 0 => (s as i64 * 1000).max(self.min_step_ms()),
            _ => super::prom::auto_step_ms(self.range.len_ms(), (self.width_px / 2).clamp(50, 1200)).max(self.min_step_ms()),
        }
    }

    fn min_step_ms(&self) -> i64 {
        (self.range.len_ms() / MAX_POINTS as i64).max(1000)
    }
}

/// Replace `$__from`, `$__to` (epoch ms), `$__from_s`, `$__to_s`, `$__interval` (`30s`), `$__interval_s`, `$__interval_ms`,
/// `$__range` (`1h`), `$__range_s`, and `$name` / `${name}` / `[[name]]` for dashboard variables. Several values of a variable
/// are joined with `|` (what a PromQL regex or a Lucene OR wants). Longest names first so `$host_name` is not `$host` + text.
pub fn interpolate(text: &str, ctx: &RunCtx, step_ms: i64) -> String {
    let mut pairs: Vec<(String, String)> = vec![
        ("__from".into(), ctx.range.from_ms.to_string()),
        ("__to".into(), ctx.range.to_ms.to_string()),
        ("__from_s".into(), (ctx.range.from_ms / 1000).to_string()),
        ("__to_s".into(), (ctx.range.to_ms / 1000).to_string()),
        ("__interval".into(), duration_text(step_ms)),
        ("__interval_s".into(), (step_ms / 1000).max(1).to_string()),
        ("__interval_ms".into(), step_ms.to_string()),
        ("__range".into(), duration_text(ctx.range.len_ms())),
        ("__range_s".into(), (ctx.range.len_ms() / 1000).to_string()),
    ];
    for (k, v) in &ctx.vars {
        pairs.push((k.clone(), v.join("|")));
    }
    pairs.sort_by_key(|p| std::cmp::Reverse(p.0.len()));
    let mut out = text.to_string();
    for (k, v) in &pairs {
        out = out.replace(&format!("${{{k}}}"), v).replace(&format!("[[{k}]]"), v);
    }
    // bare $name: only where the name ends at a non-identifier character
    let mut res = String::with_capacity(out.len());
    let chars: Vec<char> = out.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && i + 1 < chars.len() && (chars[i + 1].is_alphabetic() || chars[i + 1] == '_') {
            let mut j = i + 1;
            while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
            let name: String = chars[i + 1..j].iter().collect();
            // longest known name that is a prefix of the identifier run
            let hit = pairs.iter().find(|(k, _)| name.starts_with(k.as_str()) && (name.len() == k.len() || !k.starts_with("__")));
            match hit {
                Some((k, v)) if name.len() == k.len() => {
                    res.push_str(v);
                    i = j;
                    continue;
                }
                _ => {}
            }
        }
        res.push(chars[i]);
        i += 1;
    }
    res
}

/// The request for a panel (text after interpolation).
pub fn request_for(panel: &Panel, ctx: &RunCtx, source_default_target: Option<&str>) -> QueryRequest {
    let timeseries_like = matches!(panel.visual, Visual::Line | Visual::Area | Visual::Bar | Visual::StackedBar | Visual::Heatmap);
    let step = ctx.step_ms(panel.query.step_s);
    QueryRequest {
        text: interpolate(&panel.query.text, ctx, step),
        target: panel.query.target.clone().filter(|t| !t.is_empty()).map(|t| interpolate(&t, ctx, step)).or(source_default_target.map(String::from)),
        range: ctx.range,
        step_ms: timeseries_like.then_some(step),
        limit: if matches!(panel.visual, Visual::Table) { 2000 } else { 1000 },
        timeout_s: ctx.timeout_s,
    }
}

/// Run a panel on a source and shape the frame: series names from the legend format, hidden series dropped later by the screen.
pub fn run_panel(src: &dyn DataSource, panel: &Panel, ctx: &RunCtx, default_target: Option<&str>, cancel: &Cancel) -> Result<Frame, DashError> {
    if !panel.visual.needs_query() {
        return Ok(Frame::empty());
    }
    let req = request_for(panel, ctx, default_target);
    let mut f = src.query(&req, cancel)?;
    apply_legend(&mut f, &panel.options.legend_format);
    Ok(f)
}

pub fn apply_legend(f: &mut Frame, format: &str) {
    if format.trim().is_empty() {
        return;
    }
    for s in &mut f.series {
        s.name = series_label(format, &s.name, &s.labels);
    }
}

/// Values of a template variable: the first column of the query's rows (at most 200), or the fixed options.
pub fn variable_options(v: &Variable, src: Option<&dyn DataSource>, ctx: &RunCtx, cancel: &Cancel) -> Result<Vec<String>, DashError> {
    match &v.kind {
        VarKind::Custom { options } | VarKind::Interval { options } => Ok(options.clone()),
        VarKind::Query { query, target, .. } => {
            let src = src.ok_or_else(|| DashError::config(format!("the variable ${} needs a source", v.name)))?;
            let mut req = QueryRequest::new(&interpolate(query, ctx, 60_000), ctx.range);
            req.target = target.clone();
            req.limit = 200;
            let f = src.query(&req, cancel)?;
            let mut out: Vec<String> = if !f.rows.is_empty() {
                (0..f.rows.len()).map(|r| f.cell(r, 0)).collect()
            } else {
                // Prometheus answers `label_values`-like queries as series: use the first label's values
                f.series.iter().filter_map(|s| s.labels.values().next().cloned()).collect()
            };
            out.retain(|s| !s.is_empty());
            out.sort();
            out.dedup();
            out.truncate(200);
            Ok(out)
        }
    }
}

/// The values a variable has now (`current`, or all/first when unset).
pub fn variable_values(v: &Variable, options: &[String]) -> Vec<String> {
    let cur: Vec<String> = v.current.iter().filter(|c| options.contains(c)).cloned().collect();
    if !cur.is_empty() {
        return if v.multi { cur } else { vec![cur[0].clone()] };
    }
    if v.include_all && v.multi {
        return options.to_vec();
    }
    options.first().cloned().into_iter().collect()
}

/// Which panels reference a variable (used to refresh only those).
pub fn uses_variable(panel: &Panel, name: &str) -> bool {
    let t = &panel.query.text;
    t.contains(&format!("${name}")) || t.contains(&format!("${{{name}}}")) || t.contains(&format!("[[{name}]]"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dash::connector::fake::*;
    use crate::wsexec::FakeRunner;
    use std::sync::Arc;

    fn ctx() -> RunCtx {
        let mut c = RunCtx::new(TimeRange { from_ms: 1_792_065_600_000, to_ms: 1_792_069_200_000 });
        c.vars.insert("node".into(), vec!["a:9100".into(), "b:9100".into()]);
        c.vars.insert("node_group".into(), vec!["g".into()]);
        c
    }

    #[test]
    fn macros_and_variables_fill_in() {
        let c = ctx();
        let t = interpolate("rate(x{instance=~\"$node\"}[$__interval]) $__range ${node_group} [[node]] $__from $__to_s $unknown $nodes", &c, 30_000);
        assert_eq!(t, "rate(x{instance=~\"a:9100|b:9100\"}[30s]) 1h g a:9100|b:9100 1792065600000 1792069200 $unknown $nodes");
        assert_eq!(interpolate("$node_group $node", &c, 60_000), "g a:9100|b:9100", "longest name wins");
        assert_eq!(interpolate("no macros", &c, 1000), "no macros");
        assert_eq!(interpolate("cost: $5 and $", &c, 1000), "cost: $5 and $");
    }

    #[test]
    fn step_follows_width_and_range_and_respects_the_floor() {
        let mut c = ctx();
        assert_eq!(c.step_ms(None), 15_000, "1h over 300 points");
        c.width_px = 2000;
        assert_eq!(c.step_ms(None), 5_000);
        assert_eq!(c.step_ms(Some(60)), 60_000);
        c.range = TimeRange { from_ms: 0, to_ms: 400 * 86_400_000 };
        assert!(c.range.len_ms() / c.step_ms(Some(1)) <= MAX_POINTS as i64);
        for len in [1_000i64, 59_999, 3_600_000, 7 * 86_400_000, 90 * 86_400_000] {
            for w in [100usize, 600, 1920, 4000] {
                let mut c = ctx();
                c.range = TimeRange { from_ms: 0, to_ms: len };
                c.width_px = w;
                let n = len / c.step_ms(None) + 1;
                assert!(n <= MAX_POINTS as i64 && n <= (w as i64 / 2).clamp(50, 1200) + 2 || c.step_ms(None) == 604_800_000, "{len} {w} {n}");
            }
        }
    }

    #[test]
    fn panel_runs_with_legend_format_and_time_macros() {
        let body = r#"{"status":"success","data":{"resultType":"matrix","result":[{"metric":{"instance":"a:9100"},"values":[[1792065600,"1"]]}]}}"#;
        let up = FakeUp::new().on("query_range", 200, body);
        let d = deps(&up, &[], Arc::new(FakeRunner::new(&[])));
        let src = crate::dash::registry::build(&cfg("prometheus", "http://p:9090"), &d).unwrap();
        let p = Panel::new("p1", "CPU", "s1", Visual::Line, "rate(x{instance=~\"$node\"}[$__interval])", Grid::new(0, 0, 8, 6)).legend_format("{{instance}} cpu");
        let f = run_panel(src.as_ref(), &p, &ctx(), None, &Cancel::new()).unwrap();
        assert_eq!(f.series[0].name, "a:9100 cpu");
        let url = up.urls()[0].clone();
        assert!(url.contains("a%3A9100%7Cb%3A9100") && url.contains("step=15"), "{url}");
        // text panels never touch the network
        let t = Panel::new("p2", "Note", "", Visual::Text, "", Grid::new(0, 0, 4, 4));
        assert!(run_panel(src.as_ref(), &t, &ctx(), None, &Cancel::new()).unwrap().is_empty());
        assert_eq!(up.urls().len(), 1);
    }

    #[test]
    fn variables_pick_current_or_first() {
        let v = Variable { name: "n".into(), label: String::new(), kind: VarKind::Custom { options: vec!["a".into(), "b".into()] }, current: vec!["b".into()], multi: false, include_all: false, mapping: Default::default() };
        let opts = vec!["a".to_string(), "b".to_string()];
        assert_eq!(variable_values(&v, &opts), vec!["b"]);
        let mut w = v.clone();
        w.current = vec![];
        assert_eq!(variable_values(&w, &opts), vec!["a"]);
        w.multi = true;
        w.include_all = true;
        assert_eq!(variable_values(&w, &opts), opts);
        w.current = vec!["zzz".into()];
        assert_eq!(variable_values(&w, &opts), opts, "a stale choice falls back");
    }

    #[test]
    fn query_variables_come_from_the_first_column() {
        let body = r#"{"meta":[{"name":"host","type":"String"}],"data":[{"host":"b"},{"host":"a"},{"host":"a"},{"host":""}]}"#;
        let up = FakeUp::new().on("POST", 200, body);
        let d = deps(&up, &[], Arc::new(FakeRunner::new(&[])));
        let src = crate::dash::registry::build(&cfg("clickhouse", "http://c:8123"), &d).unwrap();
        let v = Variable { name: "host".into(), label: String::new(), kind: VarKind::Query { source: "s1".into(), query: "SELECT DISTINCT host FROM t".into(), target: None }, current: vec![], multi: false, include_all: false, mapping: Default::default() };
        assert_eq!(variable_options(&v, Some(src.as_ref()), &ctx(), &Cancel::new()).unwrap(), vec!["a", "b"]);
        assert!(variable_options(&v, None, &ctx(), &Cancel::new()).is_err());
    }
}
