//! How many tokens each model has used, kept for a year so it can be charted by day, week, month or year.
//!
//! Every answer the assistant gets from a model is counted here (the text sent and the text received, at about four
//! characters per token, because not every provider reports exact usage). The log keeps one line per call and merges
//! them into hourly totals when the system starts, so it stays small. Only the totals are kept: never what was said.


use std::sync::{Arc, Mutex};

use crate::chat::{ChatBackend, Message, ModelTurn, ToolSpec};
use crate::services::Services;
pub use iyakki_shared::usagelog::*;
use iyakki_clock::Zone;
#[cfg(test)]
use iyakki_clock::days_from_civil;
/// Wraps a model backend and counts what goes through it, for one person.
pub struct Metered {
    inner: Arc<dyn ChatBackend>,
    services: Arc<Services>,
    user: String,
}

impl Metered {
    pub fn new(inner: Arc<dyn ChatBackend>, services: Arc<Services>, user: &str) -> Metered {
        Metered { inner, services, user: user.into() }
    }

    fn note(&self, input: u64, output: u64, estimated: bool, since: std::time::Instant) {
        let (provider, model) = self.services.settings.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get().chat.as_ref().map_or(("?".to_string(), "unknown".to_string()), |c| (c.provider.clone(), c.model.clone()));
        let call = Call { src: SRC_ASSISTANT.into(), cat: SRC_ASSISTANT.into(), provider, model, input, output, estimated, failed: false, ms: since.elapsed().as_millis() as u64 };
        self.services.usage.record_call(now(), &self.user, &call);
    }
}

impl ChatBackend for Metered {
    fn provider_name(&self) -> String {
        self.inner.provider_name()
    }

    fn complete(&self, history: &[Message]) -> Result<String, String> {
        crate::providers::forget_usage();
        let t0 = std::time::Instant::now();
        let r = self.inner.complete(history)?;
        // what the provider reported when it did, the length-based estimate otherwise
        let reported = crate::providers::reported_usage();
        let (i, o) = reported.unwrap_or((crate::agent::estimate_tokens(history), (r.chars().count() as u64).div_ceil(4)));
        self.note(i, o, reported.is_none(), t0);
        Ok(r)
    }

    fn complete_tools(&self, history: &[Message], tools: &[ToolSpec]) -> Result<ModelTurn, String> {
        crate::providers::forget_usage();
        let t0 = std::time::Instant::now();
        let t = self.inner.complete_tools(history, tools)?;
        let out = crate::agent::estimate_tokens(&[Message::assistant_calls(t.text.clone(), t.calls.clone())]);
        let est_in = crate::agent::estimate_tokens(history) + tools.iter().map(|x| (x.name.len() + x.description.len() + x.schema.to_string().len()) as u64 / 4).sum::<u64>();
        let reported = crate::providers::reported_usage();
        let (i, o) = reported.unwrap_or((est_in, out));
        self.note(i, o, reported.is_none(), t0);
        Ok(t)
    }

    fn complete_tools_stream(&self, history: &[Message], tools: &[ToolSpec], on_text: &mut dyn FnMut(&str) -> bool) -> Result<ModelTurn, String> {
        crate::providers::forget_usage();
        let t0 = std::time::Instant::now();
        let t = self.inner.complete_tools_stream(history, tools, on_text)?;
        let out = crate::agent::estimate_tokens(&[Message::assistant_calls(t.text.clone(), t.calls.clone())]);
        let est_in = crate::agent::estimate_tokens(history) + tools.iter().map(|x| (x.name.len() + x.description.len() + x.schema.to_string().len()) as u64 / 4).sum::<u64>();
        let reported = crate::providers::reported_usage();
        let (i, o) = reported.unwrap_or((est_in, out));
        self.note(i, o, reported.is_none(), t0);
        Ok(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(tag: &str) -> UsageLog {
        let d = std::env::temp_dir().join(format!("iyakki-usage-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        UsageLog::open(&d.join("usage.jsonl"))
    }

    fn t(y: i64, m: u32, d: u32, h: i64) -> i64 {
        days_from_civil(y, m, d) * DAY + h * 3600
    }

    #[test]
    fn tokens_are_added_up_by_model_in_buckets_for_each_range() {
        let l = log("series");
        let z = Zone::utc();
        let now = t(2026, 10, 11, 14) + 1800;
        l.record_at(now - 600, "amy", "ollama", "qwen3:8b", 100, 50);
        l.record_at(now - 1200, "amy", "ollama", "qwen3:8b", 10, 5);
        l.record_at(now - 5 * 3600, "amy", "cloud", "big-model", 1000, 500);
        l.record_at(now - 2 * DAY, "bob", "ollama", "qwen3:8b", 7, 3);
        l.record_at(now - 40 * DAY, "amy", "cloud", "big-model", 2000, 1000);
        l.record_at(now - 200 * DAY, "amy", "cloud", "big-model", 4000, 2000);
        // a day: 24 hourly buckets, today's models only
        let day = l.series(Range::Day, now, &z, Some("amy"));
        assert_eq!(day.buckets.len(), 24);
        assert_eq!((day.input, day.output, day.calls), (1110, 555, 3));
        assert_eq!(day.models, vec!["big-model", "qwen3:8b"], "biggest first");
        assert_eq!(day.totals, vec![1500, 165]);
        assert_eq!(day.buckets[23].total(), 165, "the current hour is the last bar");
        assert_eq!(day.buckets[23].label, "14");
        // a week and a month are daily; the 2-day-old call is in both, the 40-day-old one in neither
        let week = l.series(Range::Week, now, &z, None);
        assert_eq!(week.buckets.len(), 7);
        assert_eq!(week.calls, 4);
        assert_eq!(week.buckets[6].full, "Sun 11 Oct");
        assert_eq!(week.buckets[4].total(), 10, "Fri 9 Oct");
        let month = l.series(Range::Month, now, &z, None);
        assert_eq!((month.buckets.len(), month.calls), (30, 4));
        // 13 weeks reaches back 90 days, a year 12 months
        let months = l.series(Range::Months, now, &z, Some("amy"));
        assert_eq!(months.buckets.len(), 13);
        assert_eq!(months.calls, 4, "the 40-day-old call joins");
        let year = l.series(Range::Year, now, &z, Some("amy"));
        assert_eq!(year.buckets.len(), 12);
        assert_eq!(year.calls, 5, "and the 200-day-old one");
        assert_eq!(year.buckets[11].label, "Oct");
        assert_eq!(year.buckets[0].label, "Nov", "twelve months ending in October start in November");
        // buckets tile the range without gaps or overlaps
        for r in Range::ALL {
            let b = buckets_for(r, now, &z);
            assert!(b.windows(2).all(|w| w[0].1 == w[1].0), "{r:?}");
            assert!(b.last().unwrap().1 > now && b.last().unwrap().0 <= now, "{r:?}: the last bucket holds now");
        }
        // only one person's usage when asked
        assert_eq!(l.series(Range::Week, now, &z, Some("bob")).calls, 1);
        assert_eq!(l.series(Range::Week, now, &z, Some("nobody")).calls, 0);
    }

    #[test]
    fn local_days_follow_the_time_zone_and_daylight_saving() {
        let z = Zone::from_posix("PST8PDT,M3.2.0,M11.1.0").unwrap();
        let now = t(2026, 3, 9, 20); // the day after clocks went forward
        let days = buckets_for(Range::Week, now, &z);
        assert_eq!(days.len(), 7);
        assert!(days.windows(2).all(|w| w[0].1 == w[1].0));
        assert_eq!(days[5].3, "Sun 8 Mar", "{:?}", days.iter().map(|d| d.3.clone()).collect::<Vec<_>>());
        assert_eq!(days[5].1 - days[5].0, 23 * 3600, "the short day");
        assert_eq!(days[6].3, "Mon 9 Mar");
        // a call at 23:30 local on the 8th lands on the 8th even though it is the 9th in UTC
        let e = Entry { h: (t(2026, 3, 9, 6) + 1800).div_euclid(3600), user: "a".into(), provider: "p".into(), model: "m".into(), input: 5, output: 5, calls: 1, ..Default::default() };
        let s = series_of(&[e], Range::Week, now, &z);
        assert_eq!(s.buckets[5].total(), 10, "{:?}", s.buckets.iter().map(|b| (b.full.clone(), b.total())).collect::<Vec<_>>());
    }

    #[test]
    fn lines_are_merged_into_hourly_totals_and_old_ones_dropped() {
        let d = std::env::temp_dir().join(format!("iyakki-usage-compact-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("usage.jsonl");
        let l = UsageLog::open(&p);
        let n = now();
        for _ in 0..20 {
            l.record_at(n - 60, "amy", "p", "m", 10, 5);
        }
        l.record_at(n - 500 * DAY, "amy", "p", "m", 99, 99);
        assert_eq!(std::fs::read_to_string(&p).unwrap().lines().count(), 21);
        let l2 = UsageLog::open(&p);
        let lines = std::fs::read_to_string(&p).unwrap().lines().count();
        assert!(lines <= 2, "merged into one hour (maybe two if the hour just changed): {lines}");
        let e = l2.entries(None);
        assert_eq!(e.iter().map(|x| x.calls).sum::<u64>(), 20);
        assert_eq!(e.iter().map(|x| x.input).sum::<u64>(), 200);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn old_lines_still_read_and_new_fields_stay_out_of_a_line_until_used() {
        let d = std::env::temp_dir().join(format!("iyakki-usage-compat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("usage.jsonl");
        // a line as the first versions wrote it
        std::fs::write(&p, "{\"h\":490000,\"user\":\"amy\",\"provider\":\"ollama\",\"model\":\"qwen3:8b\",\"input\":100,\"output\":50,\"calls\":2}\nnot json\n").unwrap();
        let l = UsageLog::open(&p);
        let e = l.entries(None);
        assert_eq!(e.len(), 1);
        assert_eq!((e[0].src.as_str(), e[0].errors, e[0].ms, e[0].est), ("", 0, 0, false));
        assert_eq!(source_of(&e[0]), "assistant", "a line without a source is the assistant's");
        // writing an old-style record adds nothing new to the line
        l.record_at(490_000 * 3600, "amy", "ollama", "qwen3:8b", 1, 1);
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.lines().last().unwrap().ends_with("\"calls\":1}"), "{text}");
        // a new record carries source, kind, errors and time, and reads back
        l.record_call(490_000 * 3600, "amy", &Call { src: "app:calc".into(), cat: "llm".into(), provider: "p".into(), model: "m".into(), input: 10, output: 5, estimated: true, failed: false, ms: 300 });
        let last = l.entries(None).pop().unwrap();
        assert_eq!((last.src.as_str(), last.cat.as_str(), last.ms, last.est, last.errors), ("app:calc", "llm", 300, true, 0));
        // a reader that knows nothing of the new fields (an older program) still reads the line
        let v: serde_json::Value = serde_json::from_str(std::fs::read_to_string(&p).unwrap().lines().last().unwrap()).unwrap();
        assert_eq!((v["input"].as_u64(), v["calls"].as_u64()), (Some(10), Some(1)));
    }

    #[test]
    fn calls_are_attributed_to_apps_with_errors_and_time_and_merge_by_source() {
        let l = log("attr");
        let z = Zone::utc();
        let now = t(2026, 10, 11, 14) + 1800;
        let call = |src: &str, cat: &str, i, o, failed, ms| Call { src: src.into(), cat: cat.into(), provider: "p".into(), model: "m".into(), input: i, output: o, estimated: false, failed, ms };
        l.record_call(now - 60, "amy", &call("app:calc", "llm", 100, 20, false, 200));
        l.record_call(now - 90, "amy", &call("app:calc", "llm", 50, 10, false, 400));
        l.record_call(now - 30, "amy", &call("app:calc", "llm", 999, 999, true, 100));
        l.record_call(now - 30, "amy", &call("app:calc", "tts", 40, 0, false, 900));
        l.record_call(now - 30, "amy", &call("mcp:files", "llm", 5, 5, false, 50));
        l.record_call(now - 30, "bob", &call("app:calc", "llm", 7, 3, false, 10));
        l.record_at(now - 30, "amy", "p", "m", 1000, 500);
        l.record_call(now - 3 * DAY, "amy", &call("app:calc", "llm", 1, 1, false, 5));
        let by = l.by_source(Some("amy"), now - DAY);
        let find = |src: &str, cat: &str| by.iter().find(|t| t.src == src && t.cat == cat).cloned().unwrap();
        let c = find("app:calc", "llm");
        assert_eq!((c.calls, c.input, c.output, c.errors, c.ms, c.mean_ms()), (3, 150, 30, 1, 700, 233), "a failed call counts as a call and an error, with no tokens");
        assert_eq!(find("app:calc", "tts").input, 40, "characters, not tokens");
        assert_eq!(find("assistant", "assistant").tokens(), 1500, "old-style records are the assistant's");
        assert_eq!(by.iter().map(|t| t.src.as_str()).collect::<Vec<_>>(), ["assistant", "app:calc", "app:calc", "mcp:files"], "biggest first: {by:?}");
        assert!(by.iter().all(|t| t.src != "app:other"));
        // one app, all kinds; the person filter; the range
        assert_eq!(l.totals_of(Some("amy"), "app:calc", None, now - DAY).calls, 4);
        assert_eq!(l.totals_of(Some("bob"), "app:calc", None, now - DAY).calls, 1, "another account's apps are not visible");
        assert_eq!(l.totals_of(Some("amy"), "app:calc", None, now - 5 * DAY).calls, 5);
        assert_eq!(l.totals_of(Some("amy"), "app:calc", Some("tts"), now - DAY).calls, 1, "one kind alone");
        // days: oldest first, the long name of each local day
        let days = l.days_of(Some("amy"), "app:calc", None, 7, now, &z);
        assert_eq!(days.len(), 7);
        assert_eq!(days[6].0, "Sun 11 Oct");
        assert_eq!((days[6].1.calls, days[3].1.calls), (4, 1), "{days:?}");
        assert_eq!(l.days_of(Some("amy"), "app:calc", None, 30, now, &z).len(), 30);
        assert_eq!(l.days_of(Some("amy"), "app:calc", None, 1, now, &z).len(), 1);
        // today's span in a time zone
        let (from, to) = UsageLog::today_span(now, &z);
        assert_eq!((from, to - from), (t(2026, 10, 11, 0), DAY));
        // the old charts still add everything up
        let w = l.series(Range::Week, now, &z, Some("amy"));
        assert_eq!(w.calls, 7);
        // merging keeps sources apart and adds errors and time
        let l2 = UsageLog::open(&l.path);
        let text = std::fs::read_to_string(&l2.path).unwrap();
        assert!(text.lines().count() <= 7, "{text}");
        let c2 = l2.totals_of(Some("amy"), "app:calc", None, now - DAY);
        assert_eq!((c2.calls, c2.errors, c2.ms), (4, 1, 1600));
    }

    #[test]
    fn the_assistants_calls_are_marked_and_timed() {
        use crate::chat::{ChatBackend, Message};
        struct Echo;
        impl ChatBackend for Echo {
            fn complete(&self, _: &[Message]) -> Result<String, String> {
                std::thread::sleep(std::time::Duration::from_millis(15));
                Ok("x".repeat(40))
            }
        }
        let d = std::env::temp_dir().join(format!("iyakki-usage-assistant-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        struct NoNet;
        impl iyakki_proxy::Upstream for NoNet {
            fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
                Err("offline".into())
            }
        }
        let net = std::sync::Arc::new(NoNet);
        let svc = crate::services::Services::open(&d, net.clone(), net).unwrap();
        let m = Metered::new(std::sync::Arc::new(Echo), svc.clone(), "ann");
        m.complete(&[Message::user("hello there")]).unwrap();
        let e = svc.usage.entries(Some("ann"));
        assert_eq!((e[0].src.as_str(), e[0].cat.as_str(), e[0].est, e[0].errors), ("assistant", "assistant", true, 0));
        assert!(e[0].ms >= 10, "{e:?}");
        let by = svc.usage.by_source(Some("ann"), 0);
        assert_eq!((by.len(), by[0].src.as_str(), by[0].calls), (1, "assistant", 1));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_usage_log_takes_the_providers_exact_counts_and_estimates_only_when_none_were_given() {
        use crate::chat::{ChatBackend, Message};
        struct Reporting(bool);
        impl ChatBackend for Reporting {
            fn complete(&self, _: &[Message]) -> Result<String, String> {
                // a provider client would have set these; this stands in for one
                if self.0 {
                    crate::providers::note_usage(&serde_json::json!({"usage": {"prompt_tokens": 1000, "completion_tokens": 77}}));
                }
                Ok("x".repeat(40))
            }
        }
        let d = std::env::temp_dir().join(format!("iyakki-usage-exact-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        struct NoNet;
        impl iyakki_proxy::Upstream for NoNet {
            fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
                Err("offline".into())
            }
        }
        let net = std::sync::Arc::new(NoNet);
        let svc = crate::services::Services::open(&d, net.clone(), net).unwrap();
        let exact = Metered::new(std::sync::Arc::new(Reporting(true)), svc.clone(), "ann");
        exact.complete(&[Message::user("hello there")]).unwrap();
        let guess = Metered::new(std::sync::Arc::new(Reporting(false)), svc.clone(), "ann");
        guess.complete(&[Message::user("hello there")]).unwrap();
        let ser = svc.usage.series(Range::Week, crate::clock::now_unix(), &Zone::utc(), Some("ann"));
        assert_eq!((ser.input, ser.output, ser.calls), (1000 + 6, 77 + 10, 2), "the first call is exact, the second estimated (about 4 characters a token)");
        let _ = std::fs::remove_dir_all(&d);
    }
}
