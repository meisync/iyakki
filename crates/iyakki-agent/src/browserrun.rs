//! The deterministic browser test runner (no model in the loop): a JSON spec of steps run against the person's embedded engine
//! with auto-wait and retries, ending in a pass/fail report with a screenshot of the page on failure.
//!
//! Two doors lead here:
//! * `iyakki browser-test spec.json` (and `cargo test` live tests): the person ran the spec themselves, so the domains in its
//!   `allow_domains` count as approved for the run, and the run stops as soon as the page leaves them ([`Who::Spec`]).
//! * the assistant's `browser_run_script(steps)`: the same steps, but every interacting step goes through the usual approvals
//!   ([`Who::Model`]); a model cannot type passwords, codes or card numbers this way either.
//!
//! The spec format is documented in docs/BROWSER-AGENT.md.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::browserconsent::domain_of;
use crate::browserjobs::Op;
use crate::browsertools::{Actor, Drv, Who};

#[derive(Clone, Debug)]
pub enum Step {
    Goto(String),
    Click { role: String, name: String },
    Fill { label: String, value: String },
    Check { label: String, checked: bool },
    Select { label: String, option: String },
    Press { key: String, label: Option<String> },
    WaitFor(Wait),
    Expect(Expect),
    Screenshot(String),
    SnapshotAssert(String),
}

#[derive(Clone, Debug)]
pub enum Wait {
    Text(String),
    GoneText(String),
    Element { role: String, name: String },
    Seconds(f64),
}

#[derive(Clone, Debug)]
pub enum Expect {
    Text(String),
    NoText(String),
    Url(String),
    Value { label: String, equals: String },
    Visible { role: String, name: String },
    NotVisible { role: String, name: String },
}

#[derive(Clone, Debug)]
pub struct Spec {
    pub name: String,
    pub allow_domains: Vec<String>,
    pub base_url: String,
    pub timeout_ms: u64,
    /// The spec says that its steps may pay / delete (otherwise those are refused, as for the assistant).
    pub allow_payment: bool,
    pub allow_destructive: bool,
    pub steps: Vec<(Step, Value)>,
}

fn str_of(v: &Value, what: &str) -> Result<String, String> {
    v.as_str().map(str::to_string).ok_or_else(|| format!("{what} must be text"))
}

fn field(v: &Value, k: &str, what: &str) -> Result<String, String> {
    v.get(k).and_then(Value::as_str).map(str::to_string).ok_or_else(|| format!("{what} needs \"{k}\""))
}

fn role_name(v: &Value, what: &str) -> Result<(String, String), String> {
    let role = v.get("role").and_then(Value::as_str).unwrap_or("").to_string();
    let name = v.get("name").and_then(Value::as_str).or_else(|| v.get("text").and_then(Value::as_str)).unwrap_or("").to_string();
    if role.is_empty() && name.is_empty() {
        return Err(format!("{what} needs \"role\" and/or \"name\""));
    }
    Ok((role, name))
}

pub fn parse_step(v: &Value) -> Result<Step, String> {
    let obj = v.as_object().ok_or("a step must be an object like {\"click\": {...}}")?;
    let (key, arg) = obj.iter().next().ok_or("an empty step")?;
    Ok(match key.as_str() {
        "goto" => Step::Goto(str_of(arg, "goto")?),
        "click" => {
            let (role, name) = role_name(arg, "click")?;
            Step::Click { role, name }
        }
        "fill" => Step::Fill { label: field(arg, "label", "fill")?, value: field(arg, "value", "fill")? },
        "check" => Step::Check { label: field(arg, "label", "check")?, checked: arg.get("checked").and_then(Value::as_bool).unwrap_or(true) },
        "select" => Step::Select { label: field(arg, "label", "select")?, option: field(arg, "option", "select")? },
        "press" => match arg {
            Value::String(k) => Step::Press { key: k.clone(), label: None },
            o => Step::Press { key: field(o, "key", "press")?, label: o.get("label").and_then(Value::as_str).map(str::to_string) },
        },
        "wait_for" => {
            if let Some(t) = arg.get("text").and_then(Value::as_str) {
                Step::WaitFor(Wait::Text(t.into()))
            } else if let Some(t) = arg.get("gone_text").and_then(Value::as_str) {
                Step::WaitFor(Wait::GoneText(t.into()))
            } else if let Some(e) = arg.get("element") {
                let (role, name) = role_name(e, "wait_for.element")?;
                Step::WaitFor(Wait::Element { role, name })
            } else if let Some(s) = arg.get("seconds").and_then(Value::as_f64) {
                Step::WaitFor(Wait::Seconds(s.clamp(0.0, 60.0)))
            } else {
                return Err("wait_for needs text, gone_text, element or seconds".into());
            }
        }
        "expect" => {
            if let Some(t) = arg.get("text").and_then(Value::as_str) {
                Step::Expect(Expect::Text(t.into()))
            } else if let Some(t) = arg.get("no_text").and_then(Value::as_str) {
                Step::Expect(Expect::NoText(t.into()))
            } else if let Some(u) = arg.get("url").and_then(Value::as_str) {
                Step::Expect(Expect::Url(u.into()))
            } else if let Some(v) = arg.get("value") {
                Step::Expect(Expect::Value { label: field(v, "label", "expect.value")?, equals: field(v, "equals", "expect.value")? })
            } else if let Some(v) = arg.get("visible") {
                let (role, name) = role_name(v, "expect.visible")?;
                Step::Expect(Expect::Visible { role, name })
            } else if let Some(v) = arg.get("not_visible") {
                let (role, name) = role_name(v, "expect.not_visible")?;
                Step::Expect(Expect::NotVisible { role, name })
            } else {
                return Err("expect needs text, no_text, url, value, visible or not_visible".into());
            }
        }
        "screenshot" => Step::Screenshot(str_of(arg, "screenshot")?),
        "snapshot_assert" => Step::SnapshotAssert(str_of(arg, "snapshot_assert")?),
        other => return Err(format!("unknown step \"{other}\" (goto, click, fill, check, select, press, wait_for, expect, screenshot, snapshot_assert)")),
    })
}

pub fn parse_steps(v: &Value) -> Result<Vec<(Step, Value)>, String> {
    let arr = v.as_array().ok_or("`steps` must be a list")?;
    if arr.is_empty() {
        return Err("`steps` is empty".into());
    }
    if arr.len() > 200 {
        return Err("at most 200 steps".into());
    }
    arr.iter().enumerate().map(|(i, s)| parse_step(s).map(|st| (st, s.clone())).map_err(|e| format!("step {}: {e}", i + 1))).collect()
}

pub fn parse_spec(text: &str) -> Result<Spec, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("the spec is not valid JSON: {e}"))?;
    let allow: Vec<String> = v.get("allow_domains").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(domain_of).collect()).unwrap_or_default();
    if allow.is_empty() {
        return Err("the spec needs \"allow_domains\": the sites the run may act on (running the spec is your approval for exactly these)".into());
    }
    Ok(Spec {
        name: v.get("name").and_then(Value::as_str).unwrap_or("browser test").to_string(),
        allow_domains: allow,
        base_url: v.get("base_url").and_then(Value::as_str).unwrap_or("").trim_end_matches('/').to_string(),
        timeout_ms: v.get("timeout_ms").and_then(Value::as_u64).unwrap_or(8000).clamp(500, 60_000),
        allow_payment: v.get("allow_payment").and_then(Value::as_bool).unwrap_or(false),
        allow_destructive: v.get("allow_destructive").and_then(Value::as_bool).unwrap_or(false),
        steps: parse_steps(v.get("steps").unwrap_or(&Value::Null))?,
    })
}

// ------------------------------------------------------------------------------------------------ the report

#[derive(Clone, Debug)]
pub struct StepResult {
    pub index: usize,
    pub desc: String,
    pub ok: bool,
    pub millis: u64,
    pub error: String,
    pub screenshot: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct Report {
    pub name: String,
    pub steps: Vec<StepResult>,
    pub total: usize,
    pub millis: u64,
    /// The page text at the point of failure (for the report).
    pub failure_page: String,
}

impl Report {
    pub fn passed(&self) -> bool {
        self.steps.len() == self.total && self.steps.iter().all(|s| s.ok)
    }

    pub fn text(&self) -> String {
        let ok = self.steps.iter().filter(|s| s.ok).count();
        let mut out = format!("Browser test \"{}\": {} ({ok}/{} steps, {:.1} s)\n", self.name, if self.passed() { "PASSED" } else { "FAILED" }, self.total, self.millis as f64 / 1000.0);
        for s in &self.steps {
            out.push_str(&format!("{:>3}. {} {} ({:.1} s)\n", s.index, if s.ok { "ok  " } else { "FAIL" }, s.desc, s.millis as f64 / 1000.0));
            if !s.ok {
                out.push_str(&format!("       {}\n", s.error));
                if let Some(p) = &s.screenshot {
                    out.push_str(&format!("       screenshot: {}\n", p.display()));
                }
            }
        }
        if self.steps.len() < self.total {
            out.push_str(&format!("{} step(s) were not run.\n", self.total - self.steps.len()));
        }
        if !self.failure_page.is_empty() {
            out.push_str("\nThe page at the failure:\n");
            out.push_str(&self.failure_page.lines().take(40).map(|l| format!("  {l}\n")).collect::<String>());
        }
        out
    }

    pub fn json(&self) -> Value {
        json!({
            "name": self.name, "passed": self.passed(), "steps_total": self.total, "millis": self.millis,
            "steps": self.steps.iter().map(|s| json!({"index": s.index, "desc": s.desc, "ok": s.ok, "millis": s.millis, "error": s.error, "screenshot": s.screenshot.as_ref().map(|p| p.display().to_string())})).collect::<Vec<_>>(),
        })
    }
}

fn describe_step(s: &Step) -> String {
    match s {
        Step::Goto(u) => format!("goto {u}"),
        Step::Click { role, name } => format!("click {role} \"{name}\"").replace("click  ", "click "),
        Step::Fill { label, value } => format!("fill \"{label}\" = \"{}\"", if value.chars().count() > 40 { format!("{}…", value.chars().take(40).collect::<String>()) } else { value.clone() }),
        Step::Check { label, checked } => format!("{} \"{label}\"", if *checked { "check" } else { "uncheck" }),
        Step::Select { label, option } => format!("select \"{option}\" in \"{label}\""),
        Step::Press { key, label } => format!("press {key}{}", label.as_ref().map_or(String::new(), |l| format!(" in \"{l}\""))),
        Step::WaitFor(Wait::Text(t)) => format!("wait for text \"{t}\""),
        Step::WaitFor(Wait::GoneText(t)) => format!("wait for \"{t}\" to go"),
        Step::WaitFor(Wait::Element { role, name }) => format!("wait for {role} \"{name}\""),
        Step::WaitFor(Wait::Seconds(s)) => format!("wait {s} s"),
        Step::Expect(Expect::Text(t)) => format!("expect text \"{t}\""),
        Step::Expect(Expect::NoText(t)) => format!("expect no text \"{t}\""),
        Step::Expect(Expect::Url(u)) => format!("expect url {u}"),
        Step::Expect(Expect::Value { label, equals }) => format!("expect \"{label}\" = \"{equals}\""),
        Step::Expect(Expect::Visible { role, name }) => format!("expect visible {role} \"{name}\""),
        Step::Expect(Expect::NotVisible { role, name }) => format!("expect not visible {role} \"{name}\""),
        Step::Screenshot(n) => format!("screenshot {n}"),
        Step::SnapshotAssert(t) => format!("snapshot contains \"{t}\""),
    }
}

// ------------------------------------------------------------------------------------------------ running

pub struct RunOptions {
    pub allow_domains: Vec<String>,
    pub base_url: String,
    pub timeout: Duration,
    /// Where screenshots go (created when needed).
    pub out_dir: PathBuf,
}

/// An error that may go away by waiting (the page is still changing).
fn transient(e: &str) -> bool {
    e.contains("There is no element") || e.contains("not visible") || e.contains("is disabled") || e.contains("covered") || e.contains("not found") || e.contains("No element") || e.contains("not yet") || e.contains("list of options") || e.contains("not on this page") || e.contains("the page did not answer")
}

struct Runner<'a, 'b> {
    a: &'b Actor<'a>,
    d: Drv<'a>,
    o: &'b RunOptions,
    shots: usize,
}

impl Runner<'_, '_> {
    fn deadline(&self) -> Instant {
        Instant::now() + self.o.timeout
    }

    fn url(&self, u: &str) -> String {
        if u.contains("://") || self.o.base_url.is_empty() {
            u.to_string()
        } else {
            format!("{}/{}", self.o.base_url, u.trim_start_matches('/'))
        }
    }

    /// Elements matching, retried until one (and only one) does.
    fn locate(&self, role: &str, name: &str, label: Option<&str>, roles: &[&str]) -> Result<String, String> {
        let end = self.deadline();
        let mut last = String::from("no matching element");
        loop {
            self.a.svc.browser.check_control(self.a.user)?;
            let mut q = json!({});
            if !role.is_empty() {
                q["role"] = json!(role);
            }
            if !name.is_empty() {
                q["name"] = json!(name);
            }
            if let Some(l) = label {
                q["label"] = json!(l);
            }
            if let Ok(v) = self.d.op("find", &q) {
                let all: Vec<Value> = v["matches"].as_array().cloned().unwrap_or_default();
                let mut m: Vec<&Value> = all.iter().filter(|e| roles.is_empty() || roles.contains(&e["role"].as_str().unwrap_or(""))).collect();
                if m.len() > 1 {
                    // an exact name wins over a partial one
                    let want = label.unwrap_or(name).to_lowercase();
                    let exact: Vec<&Value> = m.iter().copied().filter(|e| e["name"].as_str().unwrap_or("").to_lowercase() == want).collect();
                    if exact.len() == 1 {
                        m = exact;
                    }
                }
                match m.len() {
                    1 => return Ok(m[0]["id"].as_str().unwrap_or("").to_string()),
                    0 => last = format!("no {} \"{}\" on the page", if role.is_empty() { label.map(|_| "field").unwrap_or("element") } else { role }, label.unwrap_or(name)),
                    n => last = format!("{n} elements match \"{}\": {}. Make the name more specific", label.unwrap_or(name), m.iter().take(5).map(|e| format!("{} \"{}\"", e["role"].as_str().unwrap_or(""), e["name"].as_str().unwrap_or(""))).collect::<Vec<_>>().join(", ")),
                }
            }
            if Instant::now() >= end {
                return Err(format!("timed out waiting: {last}"));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// Run an action, again and again while it fails for a reason that waiting can fix.
    fn retry<T>(&self, mut f: impl FnMut() -> Result<T, String>) -> Result<T, String> {
        let end = self.deadline();
        loop {
            match f() {
                Ok(v) => return Ok(v),
                Err(e) if transient(&e) && Instant::now() < end => std::thread::sleep(Duration::from_millis(300)),
                Err(e) => return Err(e),
            }
        }
    }

    fn page_text(&self) -> Result<String, String> {
        let s = self.d.snapshot(40_000)?;
        Ok(s.text)
    }

    fn poll_until(&self, what: &str, mut check: impl FnMut(&Self) -> Result<bool, String>) -> Result<(), String> {
        let end = self.deadline();
        loop {
            self.a.svc.browser.check_control(self.a.user)?;
            match check(self) {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(e) if !transient(&e) => return Err(e),
                Err(_) => {}
            }
            if Instant::now() >= end {
                return Err(format!("timed out after {:.0} s waiting for {what}", self.o.timeout.as_secs_f64()));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    fn run_step(&mut self, step: &Step) -> Result<(), String> {
        match step {
            Step::Goto(u) => {
                let url = self.url(u);
                self.a.open(&url).map(|_| ())
            }
            Step::Click { role, name } => {
                let id = self.locate(role, name, None, &[])?;
                self.retry(|| self.a.click(&id)).map(|_| ())
            }
            Step::Fill { label, value } => {
                let id = self.locate("", "", Some(label), &["textbox", "combobox", "select", "slider"])?;
                let is_select = self.d.op("describe", &json!({"id": id})).ok().and_then(|v| v["el"]["native_select"].as_bool()).unwrap_or(false);
                if is_select {
                    return self.retry(|| self.a.select(&id, value)).map(|_| ());
                }
                self.retry(|| self.a.type_text(&id, value, true, false)).map(|_| ())
            }
            Step::Check { label, checked } => {
                let id = self.locate("", "", Some(label), &["checkbox", "radio"])?;
                let s = self.d.snapshot(40_000)?;
                let is = s.els.iter().find(|e| e["id"] == json!(id)).map(|e| e["state"].to_string().contains("\"checked\"")).unwrap_or(false);
                if is != *checked {
                    self.retry(|| self.a.click(&id))?;
                }
                Ok(())
            }
            Step::Select { label, option } => {
                let id = self.locate("", "", Some(label), &["select", "combobox", "listbox", "textbox"])?;
                self.retry(|| self.a.select(&id, option)).map(|_| ())
            }
            Step::Press { key, label } => {
                let id = match label {
                    Some(l) => Some(self.locate("", "", Some(l), &[])?),
                    None => None,
                };
                self.retry(|| self.a.press(key, id.as_deref())).map(|_| ())
            }
            Step::WaitFor(Wait::Seconds(s)) => {
                std::thread::sleep(Duration::from_secs_f64(*s));
                Ok(())
            }
            Step::WaitFor(Wait::Text(t)) => self.poll_until(&format!("text \"{t}\""), |r| Ok(r.page_text()?.to_lowercase().contains(&t.to_lowercase()))),
            Step::WaitFor(Wait::GoneText(t)) => self.poll_until(&format!("\"{t}\" to go away"), |r| Ok(!r.page_text()?.to_lowercase().contains(&t.to_lowercase()))),
            Step::WaitFor(Wait::Element { role, name }) => self.poll_until(&format!("{role} \"{name}\""), |r| {
                let v = r.d.op("find", &json!({"role": role, "name": name}))?;
                Ok(v["count"].as_u64().unwrap_or(0) > 0)
            }),
            Step::Expect(e) => self.expect(e),
            Step::Screenshot(name) => self.shot(name).map(|_| ()),
            Step::SnapshotAssert(t) => self.poll_until(&format!("the snapshot to contain \"{t}\""), |r| Ok(r.page_text()?.to_lowercase().contains(&t.to_lowercase()))),
        }
    }

    fn expect(&self, e: &Expect) -> Result<(), String> {
        match e {
            Expect::Text(t) => self.poll_until(&format!("text \"{t}\""), |r| Ok(r.page_text()?.to_lowercase().contains(&t.to_lowercase()))).map_err(|e| format!("expected the page to show \"{t}\": {e}")),
            Expect::NoText(t) => self.poll_until(&format!("\"{t}\" to be absent"), |r| Ok(!r.page_text()?.to_lowercase().contains(&t.to_lowercase()))).map_err(|e| format!("expected the page not to show \"{t}\": {e}")),
            Expect::Url(u) => self.poll_until(&format!("the address to contain {u}"), |r| Ok(r.d.state()?.url.contains(u.as_str()))).map_err(|e| format!("expected the address to contain {u} (it is {}): {e}", self.d.state().map(|s| s.url).unwrap_or_default())),
            Expect::Value { label, equals } => {
                let id = self.locate("", "", Some(label), &[])?;
                let mut got = String::new();
                let r = self.poll_until(&format!("\"{label}\" to be \"{equals}\""), |r| {
                    let s = r.d.snapshot(40_000)?;
                    got = s.els.iter().find(|e| e["id"] == json!(id)).and_then(|e| e["value"].as_str()).unwrap_or("").to_string();
                    Ok(&got == equals)
                });
                r.map_err(|e| format!("expected \"{label}\" to be \"{equals}\" but it is \"{got}\": {e}"))
            }
            Expect::Visible { role, name } => self.poll_until(&format!("{role} \"{name}\" to be visible"), |r| Ok(r.d.op("find", &json!({"role": role, "name": name}))?["count"].as_u64().unwrap_or(0) > 0)),
            Expect::NotVisible { role, name } => self.poll_until(&format!("{role} \"{name}\" to be gone"), |r| Ok(r.d.op("find", &json!({"role": role, "name": name}))?["count"].as_u64().unwrap_or(0) == 0)),
        }
    }

    fn shot(&mut self, name: &str) -> Result<PathBuf, String> {
        let png = match self.d.eval_png() {
            Ok(p) => p,
            Err(e) => return Err(e),
        };
        std::fs::create_dir_all(&self.o.out_dir).map_err(|e| format!("{}: {e}", self.o.out_dir.display()))?;
        self.shots += 1;
        let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(60).collect();
        let path = self.o.out_dir.join(format!("{:02}-{safe}.png", self.shots));
        std::fs::write(&path, &png).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(path)
    }

    fn inside_allowed(&self) -> Result<(), String> {
        let st = self.d.state()?;
        if st.url.is_empty() || st.url.starts_with("about:") {
            return Ok(());
        }
        let host = domain_of(&st.url);
        if self.o.allow_domains.iter().any(|d| *d == host) {
            Ok(())
        } else {
            Err(format!("the page is on {host}, which is not in allow_domains ({}); the run stops here", self.o.allow_domains.join(", ")))
        }
    }
}

impl Drv<'_> {
    pub fn eval_png(&self) -> Result<Vec<u8>, String> {
        self.st.jobs.call(self.user, Op::Screenshot, Duration::from_secs(15))?.into_png()
    }
}

/// Run the steps; stops at the first failing step (the report lists the rest as not run).
pub fn run_steps(a: &Actor, name: &str, steps: &[(Step, Value)], o: &RunOptions) -> Report {
    let start = Instant::now();
    let mut r = Runner { a, d: Drv::new(&a.svc.browser, a.user), o, shots: 0 };
    let mut results: Vec<StepResult> = Vec::new();
    let mut failure_page = String::new();
    for (i, (step, _)) in steps.iter().enumerate() {
        let t = Instant::now();
        let mut res = r.run_step(step);
        if res.is_ok() && a.who == Who::Spec {
            res = r.inside_allowed();
        }
        let ok = res.is_ok();
        let mut shot = None;
        let mut err = res.err().unwrap_or_default();
        if !ok {
            if let Ok(p) = r.shot(&format!("failure-step-{}", i + 1)) {
                shot = Some(p);
            }
            failure_page = r.d.snapshot(4000).map(|s| s.text).unwrap_or_default();
            err = err.chars().take(600).collect();
        }
        if ok {
            if let Step::Screenshot(n) = step {
                // the screenshot step already wrote its file; name it in the report
                shot = std::fs::read_dir(&o.out_dir).ok().and_then(|d| d.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.to_string_lossy().contains(&n.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect::<String>())).max());
            }
        }
        results.push(StepResult { index: i + 1, desc: describe_step(step), ok, millis: t.elapsed().as_millis() as u64, error: err, screenshot: shot });
        if !ok {
            break;
        }
    }
    Report { name: name.to_string(), total: steps.len(), steps: results, millis: start.elapsed().as_millis() as u64, failure_page }
}

/// `browser_run_script` for the assistant: the report as text (page text in it is untrusted).
pub fn run_tool(a: &Actor, args: &Value) -> Result<String, String> {
    let steps = parse_steps(args.get("steps").unwrap_or(&Value::Null))?;
    let name = args.get("name").and_then(Value::as_str).unwrap_or("script");
    let o = RunOptions { allow_domains: Vec::new(), base_url: String::new(), timeout: Duration::from_secs(8), out_dir: a.cwd.join("browser-test") };
    // a model's script may not escape the approvals: Who::Model keeps every gate
    let rep = run_steps(a, name, &steps, &o);
    let mut text = rep.text();
    if !rep.failure_page.is_empty() {
        text = text.replacen("The page at the failure:", &format!("The page at the failure ({}):", crate::browsertools::UNTRUSTED_OPEN), 1);
    }
    if rep.passed() { Ok(text) } else { Err(text) }
}

/// Run a spec file the way `iyakki browser-test` does. The caller owns the engine and the queue.
pub fn run_spec(a: &Actor, spec: &Spec, out_dir: &Path) -> Report {
    let o = RunOptions { allow_domains: spec.allow_domains.clone(), base_url: spec.base_url.clone(), timeout: Duration::from_millis(spec.timeout_ms), out_dir: out_dir.to_path_buf() };
    run_steps(a, &spec.name, &spec.steps, &o)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_are_checked_before_anything_runs() {
        assert!(parse_spec("{").unwrap_err().contains("JSON"));
        assert!(parse_spec(r#"{"steps":[{"goto":"/"}]}"#).unwrap_err().contains("allow_domains"));
        let e = parse_spec(r#"{"allow_domains":["a.org"],"steps":[{"goto":"/"},{"clickk":{}}]}"#).unwrap_err();
        assert!(e.contains("step 2") && e.contains("unknown step"), "{e}");
        let ok = parse_spec(r#"{"name":"t","allow_domains":["WWW.A.org"],"base_url":"http://a.org/","steps":[{"goto":"/x"},{"click":{"role":"button","name":"Go"}},{"fill":{"label":"Email","value":"a@b.c"}},{"select":{"label":"C","option":"US"}},{"press":"Enter"},{"wait_for":{"text":"Hi"}},{"wait_for":{"seconds":1}},{"expect":{"value":{"label":"Email","equals":"a@b.c"}}},{"expect":{"visible":{"role":"heading","name":"Hi"}}},{"screenshot":"s"},{"snapshot_assert":"Hi"}]}"#).unwrap();
        assert_eq!(ok.allow_domains, vec!["a.org"]);
        assert_eq!(ok.base_url, "http://a.org");
        assert_eq!(ok.steps.len(), 11);
        assert!(parse_steps(&json!([])).is_err());
        assert!(parse_step(&json!({"expect": {}})).is_err());
        assert!(parse_step(&json!({"click": {}})).is_err());
    }

    #[test]
    fn the_report_lists_steps_and_the_failure() {
        let r = Report { name: "x".into(), total: 3, steps: vec![StepResult { index: 1, desc: "goto /".into(), ok: true, millis: 100, error: String::new(), screenshot: None }, StepResult { index: 2, desc: "click button \"Go\"".into(), ok: false, millis: 8000, error: "timed out".into(), screenshot: Some("/tmp/f.png".into()) }], millis: 8100, failure_page: "Page \"x\"".into() };
        assert!(!r.passed());
        let t = r.text();
        assert!(t.contains("FAILED (1/3 steps") && t.contains("FAIL click button") && t.contains("screenshot: /tmp/f.png") && t.contains("1 step(s) were not run"), "{t}");
        assert_eq!(r.json()["passed"], json!(false));
    }
}
