//! `setTimeout`, `setInterval`, `requestAnimationFrame`, `queueMicrotask`, `performance.now`.
//!
//! Time here is virtual: the page's timers run in order, as fast as the engine can, up to a budget of virtual
//! milliseconds and callbacks. A page that waits two seconds before showing its content shows it at once, and one that
//! polls forever is stopped at the budget.

use std::any::Any;

use crate::engine::{JsEngine, JsError, WebModule};

pub struct TimersModule;

impl WebModule for TimersModule {
    fn name(&self) -> &'static str {
        "timers"
    }

    fn prelude(&self) -> &'static str {
        r#"
(function (g) {
  'use strict';
  let seq = 0, now = 0;
  const q = [];
  const report = (e) => { try { __host('console', 'log', JSON.stringify({ level: 'error', text: 'Uncaught ' + String(e) + (e && e.stack ? '\n' + e.stack : '') })); } catch (_) {} };
  function add(fn, ms, args, every) {
    if (typeof fn !== 'function') { const src = String(fn); fn = () => (0, eval)(src); }
    ms = Math.max(0, +ms || 0);
    const id = ++seq;
    q.push({ id, due: now + ms, order: id, fn, args, every: every ? Math.max(1, ms) : 0 });
    return id;
  }
  g.setTimeout = (fn, ms, ...a) => add(fn, ms, a, false);
  g.setInterval = (fn, ms, ...a) => add(fn, ms, a, true);
  g.setImmediate = (fn, ...a) => add(fn, 0, a, false);
  g.clearTimeout = g.clearInterval = g.clearImmediate = (id) => { const i = q.findIndex((t) => t.id === id); if (i >= 0) q.splice(i, 1); };
  g.requestAnimationFrame = (fn) => add(() => fn(now), 16, [], false);
  g.cancelAnimationFrame = g.clearTimeout;
  g.requestIdleCallback = (fn) => add(() => fn({ didTimeout: false, timeRemaining: () => 50 }), 1, [], false);
  g.cancelIdleCallback = g.clearTimeout;
  if (typeof g.queueMicrotask !== 'function') g.queueMicrotask = (fn) => { Promise.resolve().then(fn); };
  {
    // Navigation Timing and friends: pages read these to report load speed, and fail when they are missing
    const origin = Date.now();
    const timing = {
      navigationStart: origin, unloadEventStart: 0, unloadEventEnd: 0, redirectStart: 0, redirectEnd: 0, fetchStart: origin, domainLookupStart: origin, domainLookupEnd: origin,
      connectStart: origin, connectEnd: origin, secureConnectionStart: origin, requestStart: origin, responseStart: origin, responseEnd: origin, domLoading: origin,
      domInteractive: origin, domContentLoadedEventStart: origin, domContentLoadedEventEnd: origin, domComplete: origin, loadEventStart: origin, loadEventEnd: origin,
      toJSON() { return Object.assign({}, this); },
    };
    const navEntry = () => ({
      name: String(g.location && g.location.href || ''), entryType: 'navigation', startTime: 0, duration: 0, initiatorType: 'navigation', nextHopProtocol: 'h2',
      workerStart: 0, redirectStart: 0, redirectEnd: 0, fetchStart: 0, domainLookupStart: 0, domainLookupEnd: 0, connectStart: 0, connectEnd: 0, secureConnectionStart: 0,
      requestStart: 0, responseStart: 0, responseEnd: 0, transferSize: 0, encodedBodySize: 0, decodedBodySize: 0, domInteractive: 0, domContentLoadedEventStart: 0,
      domContentLoadedEventEnd: 0, domComplete: 0, loadEventStart: 0, loadEventEnd: 0, unloadEventStart: 0, unloadEventEnd: 0, type: 'navigate', redirectCount: 0,
      toJSON() { return Object.assign({}, this); },
    });
    const entries = [];
    const perf = {
      now: () => now, timeOrigin: origin, timing, navigation: { type: 0, redirectCount: 0, TYPE_NAVIGATE: 0, TYPE_RELOAD: 1, TYPE_BACK_FORWARD: 2 },
      memory: { jsHeapSizeLimit: 2147483648, totalJSHeapSize: 10000000, usedJSHeapSize: 10000000 },
      eventCounts: new Map(),
      mark(name, o) { const e = { name: String(name), entryType: 'mark', startTime: now, duration: 0, detail: (o && o.detail) || null }; entries.push(e); return e; },
      measure(name, a, b) { const e = { name: String(name), entryType: 'measure', startTime: 0, duration: now, detail: null }; entries.push(e); return e; },
      clearMarks(n) { for (let i = entries.length - 1; i >= 0; i--) if (entries[i].entryType === 'mark' && (n === undefined || entries[i].name === n)) entries.splice(i, 1); },
      clearMeasures(n) { for (let i = entries.length - 1; i >= 0; i--) if (entries[i].entryType === 'measure' && (n === undefined || entries[i].name === n)) entries.splice(i, 1); },
      clearResourceTimings() {}, setResourceTimingBufferSize() {},
      getEntries() { return [navEntry()].concat(entries); },
      getEntriesByType(t) { return t === 'navigation' ? [navEntry()] : entries.filter((e) => e.entryType === t); },
      getEntriesByName(n, t) { return this.getEntries().filter((e) => e.name === n && (t === undefined || e.entryType === t)); },
      toJSON() { return { timeOrigin: origin, timing }; },
    };
    g.performance = perf;
    class PerformanceObserver {
      constructor(cb) { this._cb = cb; }
      observe() {} disconnect() {} takeRecords() { return []; }
      static get supportedEntryTypes() { return ['navigation', 'resource', 'mark', 'measure', 'paint']; }
    }
    g.PerformanceObserver = PerformanceObserver;
    g.PerformanceEntry = class PerformanceEntry {}; g.PerformanceNavigationTiming = class PerformanceNavigationTiming {};
  }
  g.__timers = {
    // run the earliest timer due at or before `limit` (virtual ms): 1 = ran one, 0 = nothing due
    step(limit) {
      if (!q.length) return 0;
      let best = 0;
      for (let i = 1; i < q.length; i++) if (q[i].due < q[best].due || (q[i].due === q[best].due && q[i].order < q[best].order)) best = i;
      const t = q[best];
      if (t.due > limit) return 0;
      q.splice(best, 1);
      if (t.due > now) now = t.due;
      if (t.every) { t.due = now + t.every; t.order = ++seq; q.push(t); }
      try { t.fn(...t.args); } catch (e) { report(e); }
      return 1;
    },
    pending() { return q.length; },
    now() { return now; },
  };
})(globalThis);
"#
    }

    fn call(&mut self, op: &str, _args: &serde_json::Value) -> Result<serde_json::Value, String> {
        Err(format!("timers has no operation {op}"))
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimerReport {
    /// Callbacks that ran.
    pub ran: u32,
    /// Timers still waiting (beyond the budget, or intervals).
    pub pending: u32,
    /// Virtual milliseconds that passed.
    pub virtual_ms: u64,
}

/// Run the page's timers, and the promise reactions each one leaves behind, within a budget.
pub fn run_timers(e: &mut JsEngine, budget_ms: u64, max_callbacks: u32) -> Result<TimerReport, JsError> {
    let start: f64 = e.eval("timers", "__timers.now()")?.parse().unwrap_or(0.0);
    let limit = start + budget_ms as f64;
    let mut ran = 0;
    e.run_jobs()?;
    while ran < max_callbacks {
        let did = e.eval("timers", &format!("__timers.step({limit})"))?;
        if did != "1" {
            break;
        }
        ran += 1;
        e.run_jobs()?;
    }
    let now: f64 = e.eval("timers", "__timers.now()")?.parse().unwrap_or(start);
    let pending: u32 = e.eval("timers", "__timers.pending()")?.parse().unwrap_or(0);
    Ok(TimerReport { ran, pending, virtual_ms: (now - start).max(0.0) as u64 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::console::ConsoleModule;
    use crate::Limits;

    fn engine() -> JsEngine {
        JsEngine::new(Limits::default(), vec![Box::new(ConsoleModule::new()), Box::new(TimersModule)]).unwrap()
    }

    fn lines(e: &mut JsEngine) -> Vec<String> {
        e.module::<ConsoleModule>("console").unwrap().lines.iter().map(|l| l.text.clone()).collect()
    }

    #[test]
    fn timers_run_in_order_of_time_then_creation_and_virtual_time_skips_the_waiting() {
        let mut e = engine();
        e.eval("t", "setTimeout(() => console.log('c'), 500); setTimeout(() => console.log('a'), 0); setTimeout(() => console.log('b'), 100); setTimeout(() => console.log('a2'), 0); console.log('sync')").unwrap();
        let r = run_timers(&mut e, 10_000, 100).unwrap();
        assert_eq!(lines(&mut e), vec!["sync", "a", "a2", "b", "c"]);
        assert_eq!((r.ran, r.pending, r.virtual_ms), (4, 0, 500), "half a second of waiting took no time");
    }

    #[test]
    fn promises_settle_between_timers_and_timers_can_chain() {
        let mut e = engine();
        e.eval("t", "setTimeout(() => { console.log('t1'); Promise.resolve().then(() => console.log('micro after t1')); setTimeout(() => console.log('t2'), 10); }, 5); queueMicrotask(() => console.log('first micro'))").unwrap();
        run_timers(&mut e, 1000, 100).unwrap();
        assert_eq!(lines(&mut e), vec!["first micro", "t1", "micro after t1", "t2"]);
    }

    #[test]
    fn clearing_works_and_intervals_stop_at_the_budget() {
        let mut e = engine();
        e.eval("t", "const id = setTimeout(() => console.log('never'), 10); clearTimeout(id); let n = 0; const iv = setInterval(() => { n++; if (n === 3) clearInterval(iv); }, 100); setInterval(() => console.log('tick'), 1000)").unwrap();
        let r = run_timers(&mut e, 3500, 1000).unwrap();
        assert_eq!(lines(&mut e), vec!["tick", "tick", "tick"], "the endless interval ran only within the budget");
        assert_eq!(r.pending, 1);
        assert_eq!(e.eval("t", "n").unwrap(), "3");
    }

    #[test]
    fn a_page_that_polls_forever_is_stopped_by_the_callback_limit() {
        let mut e = engine();
        e.eval("t", "setInterval(() => {}, 1)").unwrap();
        let r = run_timers(&mut e, 1_000_000, 50).unwrap();
        assert_eq!(r.ran, 50);
        assert_eq!(r.pending, 1);
    }

    #[test]
    fn a_throwing_callback_is_reported_and_the_rest_still_run() {
        let mut e = engine();
        e.eval("t", "setTimeout(() => { throw new Error('oops'); }, 1); setTimeout(() => console.log('after'), 2)").unwrap();
        run_timers(&mut e, 100, 10).unwrap();
        let l = lines(&mut e);
        assert!(l[0].starts_with("Uncaught Error: oops") && l[1] == "after", "{l:?}");
    }

    #[test]
    fn navigation_timing_and_performance_observers_exist() {
        let mut e = engine();
        e.eval("t", "const n = performance.getEntriesByType('navigation')[0]; console.log([typeof n.requestStart, typeof performance.timing.requestStart, typeof performance.timing.navigationStart, performance.getEntriesByType('paint').length, typeof PerformanceObserver, PerformanceObserver.supportedEntryTypes.length > 0].join(' ')); performance.mark('a'); console.log(performance.getEntriesByName('a').length + ' ' + performance.getEntriesByType('mark').length); new PerformanceObserver(() => {}).observe({ entryTypes: ['navigation'] });").unwrap();
        e.run_jobs().unwrap();
        let got = e.module::<ConsoleModule>("console").unwrap().lines.iter().map(|l| l.text.clone()).collect::<Vec<_>>();
        assert_eq!(got, vec!["number number number 0 function true", "1 1"]);
    }

    #[test]
    fn string_timers_animation_frames_and_performance_now() {
        let mut e = engine();
        e.eval("t", "setTimeout('console.log(\"from a string\")', 1); requestAnimationFrame((t) => console.log('frame ' + (t >= 16))); setTimeout(() => console.log('now ' + performance.now()), 40)").unwrap();
        run_timers(&mut e, 100, 10).unwrap();
        assert_eq!(lines(&mut e), vec!["from a string", "frame true", "now 40"]);
    }
}
