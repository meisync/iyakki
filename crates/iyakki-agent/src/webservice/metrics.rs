//! `GET /metrics` (F-WEBSVC-10): the service's own numbers as Prometheus text, written by hand, and the structured log line.
//!
//! Counters: requests by route and status class, the time they took (sum and count per route), sign-ins by outcome. Gauges read at
//! the moment of the scrape: sessions, connections, run states, process uptime. Route names are the table's ids and status is the
//! class (2xx, 3xx, 4xx, 5xx), so a scanner cannot make the number of series grow: unknown routes are 404 before they get here and
//! are counted under the fixed label `unknown`. Never a user name, an address, a token, a path parameter or a prompt.
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Instant;

pub struct Metrics {
    started: Instant,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// (route id, status class) -> count
    requests: BTreeMap<(String, &'static str), u64>,
    /// route id -> (seconds summed in microseconds, count)
    timing: BTreeMap<String, (u64, u64)>,
    refused_rate: u64,
    login_ok: u64,
    login_failed: u64,
    /// What happened, by a fixed name and an optional fixed outcome (never a name, an address or a path parameter).
    events: BTreeMap<(&'static str, &'static str), u64>,
}

fn class(status: u16) -> &'static str {
    match status {
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        _ => "5xx",
    }
}

/// A label value that cannot break the line: letters, digits, underscore and dot only (route ids already are).
fn label(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')).take(40).collect()
}

/// What the scrape adds that the counters do not hold.
#[derive(Default, Clone, Copy)]
pub struct Gauges {
    pub sessions: usize,
    pub connections: usize,
    pub runs: usize,
    /// Runs held in memory by state (`running`, `completed`, `failed`, `stopped`) and the tool results they hold, good and failed.
    pub runs_by_status: [(&'static str, usize); 4],
    pub tool_results_ok: usize,
    pub tool_results_failed: usize,
    /// Tool calls waiting for a person's answer.
    pub approvals_pending: usize,
    /// The resident memory of the service and the free space of the data folder, when this machine says (Linux and the other systems with
    /// the files; `None` leaves the line out).
    pub memory_bytes: Option<u64>,
    pub data_free_bytes: Option<u64>,
    /// Running totals since the service started (they do not fall when a run is forgotten): tool calls that came back, by outcome, and the
    /// tokens the models used.
    pub tool_calls_ok_total: u64,
    pub tool_calls_failed_total: u64,
    pub model_tokens_total: u64,
}

/// The resident memory of this process (Linux: `/proc/self/status`); `None` where the system does not say.
pub fn resident_memory() -> Option<u64> {
    iyakki_core_local::procfacts::resident_bytes()
}

impl Default for Metrics {
    fn default() -> Metrics {
        Metrics::new()
    }
}

impl Metrics {
    pub fn new() -> Metrics {
        Metrics { started: Instant::now(), inner: Mutex::new(Inner::default()) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// One answered request. `route` is the table id (`unknown` when the request matched no route).
    pub fn request(&self, route: &str, status: u16, micros: u64) {
        let r = label(route);
        let mut g = self.lock();
        *g.requests.entry((r.clone(), class(status))).or_insert(0) += 1;
        let t = g.timing.entry(r).or_insert((0, 0));
        t.0 += micros;
        t.1 += 1;
        if status == 429 {
            g.refused_rate += 1;
        }
    }

    /// Counts what a successful call stands for (a run started, a flow run started, an approval answered, a tile event), by the route's id.
    pub fn event(&self, route: &str, status: u16) {
        if !(200..300).contains(&status) {
            return;
        }
        let key = match route {
            "runs_start" => ("runs_started", ""),
            "runs_stop" => ("runs_stop_requested", ""),
            "approvals_decide" => ("approvals_answered", "tool_call"),
            "tapprovals_approve" => ("approvals_answered", "tenant_approved"),
            "tapprovals_deny" => ("approvals_answered", "tenant_denied"),
            "flows_run" => ("flow_runs_started", ""),
            "tile_events" => ("tile_events", ""),
            _ => return,
        };
        *self.lock().events.entry(key).or_insert(0) += 1;
    }

    pub fn login(&self, ok: bool) {
        let mut g = self.lock();
        if ok {
            g.login_ok += 1;
        } else {
            g.login_failed += 1;
        }
    }

    /// The Prometheus text exposition format, version 0.0.4.
    pub fn render(&self, profile: &str, version: &str, gauges: Gauges) -> String {
        let g = self.lock();
        let mut s = String::new();
        let meta = |s: &mut String, name: &str, help: &str, kind: &str| {
            s.push_str(&format!("# HELP {name} {help}\n# TYPE {name} {kind}\n"));
        };
        meta(&mut s, "iyakki_build_info", "The profile and version of this service (always 1).", "gauge");
        s.push_str(&format!("iyakki_build_info{{profile=\"{}\",version=\"{}\"}} 1\n", label(profile), label(version)));
        meta(&mut s, "iyakki_uptime_seconds", "Seconds since the service started.", "gauge");
        s.push_str(&format!("iyakki_uptime_seconds {}\n", self.started.elapsed().as_secs()));
        meta(&mut s, "iyakki_sessions", "Signed-in sessions (tokens) that have not ended.", "gauge");
        s.push_str(&format!("iyakki_sessions {}\n", gauges.sessions));
        meta(&mut s, "iyakki_connections_open", "Connections being served now.", "gauge");
        s.push_str(&format!("iyakki_connections_open {}\n", gauges.connections));
        meta(&mut s, "iyakki_runs", "Chat runs the service holds in memory (running or kept for reading).", "gauge");
        s.push_str(&format!("iyakki_runs {}\n", gauges.runs));
        meta(&mut s, "iyakki_runs_held", "Chat runs held in memory, by state.", "gauge");
        for (st, n) in gauges.runs_by_status {
            s.push_str(&format!("iyakki_runs_held{{status=\"{st}\"}} {n}\n"));
        }
        meta(&mut s, "iyakki_tool_results_held", "Tool results in the runs held in memory, by outcome (a gauge: runs are forgotten).", "gauge");
        s.push_str(&format!("iyakki_tool_results_held{{outcome=\"ok\"}} {}\niyakki_tool_results_held{{outcome=\"failed\"}} {}\n", gauges.tool_results_ok, gauges.tool_results_failed));
        meta(&mut s, "iyakki_approvals_pending", "Tool calls waiting for a person's answer.", "gauge");
        s.push_str(&format!("iyakki_approvals_pending {}\n", gauges.approvals_pending));
        if let Some(b) = gauges.memory_bytes {
            meta(&mut s, "iyakki_process_resident_memory_bytes", "Resident memory of the service.", "gauge");
            s.push_str(&format!("iyakki_process_resident_memory_bytes {b}\n"));
        }
        if let Some(b) = gauges.data_free_bytes {
            meta(&mut s, "iyakki_data_free_bytes", "Free space of the data folder's disk.", "gauge");
            s.push_str(&format!("iyakki_data_free_bytes {b}\n"));
        }
        meta(&mut s, "iyakki_events_total", "Things that happened, counted when the call succeeded: runs started, runs asked to stop, approvals answered, flow runs started, tile events.", "counter");
        for ((name, outcome), n) in &g.events {
            if outcome.is_empty() {
                s.push_str(&format!("iyakki_events_total{{event=\"{name}\"}} {n}\n"));
            } else {
                s.push_str(&format!("iyakki_events_total{{event=\"{name}\",outcome=\"{outcome}\"}} {n}\n"));
            }
        }
        meta(&mut s, "iyakki_tool_calls_total", "Tool calls that came back to a chat run since the service started, by outcome (a counter: runs are forgotten, this is not).", "counter");
        s.push_str(&format!("iyakki_tool_calls_total{{outcome=\"ok\"}} {}\niyakki_tool_calls_total{{outcome=\"failed\"}} {}\n", gauges.tool_calls_ok_total, gauges.tool_calls_failed_total));
        meta(&mut s, "iyakki_model_tokens_total", "Tokens the models used in chat runs since the service started.", "counter");
        s.push_str(&format!("iyakki_model_tokens_total {}\n", gauges.model_tokens_total));
        meta(&mut s, "iyakki_requests_total", "Requests answered, by route and status class.", "counter");
        for ((r, c), n) in &g.requests {
            s.push_str(&format!("iyakki_requests_total{{route=\"{r}\",status=\"{c}\"}} {n}\n"));
        }
        meta(&mut s, "iyakki_request_duration_seconds", "Time spent answering, by route (sum and count).", "summary");
        for (r, (us, n)) in &g.timing {
            s.push_str(&format!("iyakki_request_duration_seconds_sum{{route=\"{r}\"}} {:.6}\niyakki_request_duration_seconds_count{{route=\"{r}\"}} {n}\n", *us as f64 / 1e6));
        }
        meta(&mut s, "iyakki_rate_limited_total", "Requests refused for coming too fast.", "counter");
        s.push_str(&format!("iyakki_rate_limited_total {}\n", g.refused_rate));
        meta(&mut s, "iyakki_logins_total", "Sign-ins by outcome.", "counter");
        s.push_str(&format!("iyakki_logins_total{{outcome=\"ok\"}} {}\niyakki_logins_total{{outcome=\"failed\"}} {}\n", g.login_ok, g.login_failed));
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_counted_by_route_and_class_and_the_text_is_valid_exposition() {
        let m = Metrics::new();
        m.request("whoami", 200, 1500);
        m.request("whoami", 200, 500);
        m.request("whoami", 401, 100);
        m.request("login", 429, 10);
        m.request("a\"b\nc{x}", 500, 1);
        m.login(true);
        m.login(false);
        m.login(false);
        for (r, st) in [("runs_start", 200), ("runs_start", 200), ("runs_start", 429), ("approvals_decide", 200), ("tapprovals_deny", 200), ("flows_run", 200), ("whoami", 200)] {
            m.event(r, st);
        }
        let t = m.render("cloud", "0.1.0-test", Gauges { sessions: 3, connections: 2, runs: 1, runs_by_status: [("running", 1), ("completed", 4), ("failed", 0), ("stopped", 2)], tool_results_ok: 9, tool_results_failed: 1, approvals_pending: 2, memory_bytes: Some(1234), data_free_bytes: None, tool_calls_ok_total: 7, tool_calls_failed_total: 2, model_tokens_total: 4321 });
        assert!(t.contains("iyakki_events_total{event=\"runs_started\"} 2\n") && t.contains("iyakki_events_total{event=\"approvals_answered\",outcome=\"tool_call\"} 1\n") && t.contains("outcome=\"tenant_denied\"} 1\n") && t.contains("iyakki_events_total{event=\"flow_runs_started\"} 1\n"), "{t}");
        assert!(!t.contains("whoami\"} 1\n") || !t.contains("event=\"whoami"), "only the named routes are events");
        assert!(t.contains("iyakki_runs_held{status=\"completed\"} 4\n") && t.contains("iyakki_tool_results_held{outcome=\"failed\"} 1\n") && t.contains("iyakki_approvals_pending 2\n") && t.contains("iyakki_process_resident_memory_bytes 1234\n"), "{t}");
        assert!(!t.contains("iyakki_data_free_bytes"), "a number the machine did not give is left out");
        assert!(t.contains("iyakki_requests_total{route=\"whoami\",status=\"2xx\"} 2\n"), "{t}");
        assert!(t.contains("iyakki_requests_total{route=\"whoami\",status=\"4xx\"} 1\n"));
        assert!(t.contains("iyakki_requests_total{route=\"login\",status=\"4xx\"} 1\n") && t.contains("iyakki_rate_limited_total 1\n"));
        assert!(t.contains("iyakki_request_duration_seconds_sum{route=\"whoami\"} 0.002100\n") && t.contains("iyakki_request_duration_seconds_count{route=\"whoami\"} 3\n"));
        assert!(t.contains("iyakki_logins_total{outcome=\"ok\"} 1\n") && t.contains("iyakki_logins_total{outcome=\"failed\"} 2\n"));
        assert!(t.contains("iyakki_sessions 3\n") && t.contains("iyakki_connections_open 2\n") && t.contains("iyakki_runs 1\n"));
        assert!(t.contains("iyakki_build_info{profile=\"cloud\",version=\"0.1.0-test\"} 1\n"));
        // a hostile route name cannot break a line or add a label
        assert!(t.contains("route=\"abcx\""), "{t}");
        // every sample line is `name{labels} value` or `name value`, every family has HELP and TYPE first
        for l in t.lines() {
            if l.starts_with('#') {
                assert!(l.starts_with("# HELP ") || l.starts_with("# TYPE "), "{l}");
            } else {
                let (name_labels, value) = l.rsplit_once(' ').unwrap();
                assert!(value.parse::<f64>().is_ok(), "{l}");
                assert!(name_labels.matches('{').count() == name_labels.matches('}').count() && name_labels.matches('"').count() % 2 == 0, "{l}");
            }
        }
        assert!(t.ends_with('\n'));
    }
}
