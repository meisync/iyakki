//! Test containers: the UNIT-TEST image (`deploy_build` with a test target: the tests and coverage run while the image builds) and the
//! INTEGRATION-TEST Job (`deploy_run_job`: run an image as a Kubernetes Job after the rollout, wait, read the logs, delete it).
//! [`parse_outcome`] reads pass/fail counts and a coverage percentage out of npm/jest, pytest, go test and cargo test output.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{arg, cut, first_line, object_name, tail_lines, K8sTools};

/// A stage that runs tests (test, unit-test, integration-test, coverage): built into its own image, never deployed.
pub fn is_test_target(t: &str) -> bool {
    let t = t.to_lowercase();
    !t.is_empty() && (t.contains("test") || t.contains("cover"))
}

/// `--build-arg K=V` for each entry of the `build_args` object.
pub fn build_args(args: &Value) -> Result<Vec<String>, String> {
    let Some(o) = args.get("build_args").and_then(Value::as_object) else { return Ok(vec![]) };
    let mut v = Vec::new();
    for (k, val) in o {
        if k.is_empty() || k.len() > 64 || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || k.starts_with(|c: char| c.is_ascii_digit()) {
            return Err(format!("`{k}` is not a build argument name (letters, digits and _)"));
        }
        let text = match val {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            _ => return Err(format!("build argument {k} must be a string, number or boolean")),
        };
        if text.len() > 500 || text.chars().any(char::is_control) {
            return Err(format!("the value of build argument {k} is too long or has control characters"));
        }
        v.push("--build-arg".to_string());
        v.push(format!("{k}={text}"));
    }
    Ok(v)
}

/// What a Job run came to, as data.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JobRun {
    pub job: String,
    pub pod: String,
    pub passed: bool,
    pub failed: bool,
    pub timed_out: bool,
    pub exit_code: Option<i32>,
    pub logs: String,
    pub took_s: u64,
    pub namespace: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    /// (passed, failed)
    pub counts: Option<(u64, u64)>,
    pub coverage: Option<f64>,
}

fn number_before(line: &str, word: &str) -> Option<u64> {
    let i = line.find(word)?;
    line[..i].trim_end().rsplit(|c: char| !c.is_ascii_digit()).next().and_then(|n| n.parse().ok())
}

fn percent_in(s: &str) -> Option<f64> {
    s.split_whitespace().find_map(|t| t.trim_end_matches([',', ')', ';']).strip_suffix('%').and_then(|n| n.parse::<f64>().ok()))
}

/// Pass/fail counts and a coverage % from test output (jest, pytest, go test, cargo test).
pub fn parse_outcome(text: &str) -> Outcome {
    let mut out = Outcome::default();
    // jest / npm: "Tests:       1 failed, 5 passed, 6 total"
    if let Some(l) = text.lines().find(|l| l.trim_start().starts_with("Tests:") && l.contains("total")) {
        out.counts = Some((number_before(l, "passed").unwrap_or(0), number_before(l, "failed").unwrap_or(0)));
    }
    // cargo: "test result: ok. 12 passed; 0 failed; 0 ignored" (one per test binary)
    if out.counts.is_none() {
        let (mut p, mut f, mut any) = (0, 0, false);
        for l in text.lines().filter(|l| l.contains("test result:")) {
            any = true;
            p += number_before(l, " passed").unwrap_or(0);
            f += number_before(l, " failed").unwrap_or(0);
        }
        if any {
            out.counts = Some((p, f));
        }
    }
    // pytest: "== 5 passed, 1 failed in 0.12s ==" or "5 passed in 0.10s"
    if out.counts.is_none() {
        if let Some(l) = text.lines().rev().find(|l| (l.contains(" passed") || l.contains(" failed") || l.contains(" error")) && l.contains(" in ") && (l.trim_start().starts_with('=') || l.trim_end().ends_with(')') || l.contains("s =") || l.trim_end().ends_with('s'))) {
            out.counts = Some((number_before(l, " passed").unwrap_or(0), number_before(l, " failed").unwrap_or(0) + number_before(l, " error").unwrap_or(0)));
        }
    }
    // go test: "--- PASS:" / "--- FAIL:" with -v, else the packages: "ok  \tpkg\t0.01s" / "FAIL\tpkg"
    if out.counts.is_none() {
        let p = text.lines().filter(|l| l.trim_start().starts_with("--- PASS:")).count() as u64;
        let f = text.lines().filter(|l| l.trim_start().starts_with("--- FAIL:")).count() as u64;
        if p + f > 0 {
            out.counts = Some((p, f));
        } else {
            let p = text.lines().filter(|l| l.starts_with("ok ") || l.starts_with("ok\t")).count() as u64;
            let f = text.lines().filter(|l| l.starts_with("FAIL\t") || l.starts_with("FAIL ")).count() as u64;
            if p + f > 0 {
                out.counts = Some((p, f));
            }
        }
    }
    // coverage: istanbul/jest "All files | 87.5 | ...", coverage.py "TOTAL  120  10  91%", go "coverage: 78.2% of statements"
    if let Some(l) = text.lines().find(|l| l.contains("All files") && l.contains('|')) {
        out.coverage = l.split('|').nth(1).and_then(|c| c.trim().parse::<f64>().ok());
    }
    if out.coverage.is_none() {
        if let Some(l) = text.lines().find(|l| l.trim_start().starts_with("TOTAL") && l.trim_end().ends_with('%')) {
            out.coverage = l.split_whitespace().last().and_then(|t| t.trim_end_matches('%').parse().ok());
        }
    }
    if out.coverage.is_none() {
        let v: Vec<f64> = text.lines().filter(|l| l.contains("coverage:") && l.contains("% of statements")).filter_map(|l| l.split("coverage:").nth(1).and_then(percent_in)).collect();
        if !v.is_empty() {
            out.coverage = Some((v.iter().sum::<f64>() / v.len() as f64 * 10.0).round() / 10.0);
        }
    }
    if out.coverage.is_none() {
        out.coverage = text.lines().filter(|l| l.to_lowercase().contains("coverage")).find_map(|l| l.split(':').nth(1).and_then(percent_in));
    }
    out
}

impl Outcome {
    pub fn summary(&self) -> String {
        let mut v = Vec::new();
        if let Some((p, f)) = self.counts {
            v.push(format!("Tests: {p} passed, {f} failed"));
        }
        if let Some(c) = self.coverage {
            v.push(format!("Coverage: {c}%"));
        }
        if v.is_empty() {
            "No pass/fail count or coverage line was recognised (npm/jest, pytest, go test and cargo test summaries are read); read the output below.".to_string()
        } else {
            v.join(". ")
        }
    }
}

fn tail_chars(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut start = s.len() - n;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    format!("… (earlier output cut)\n{}", &s[start..])
}

fn yq(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

impl K8sTools {
    /// Test images (built with a test target) are never loaded or pushed.
    pub(super) fn is_test_image(&self, r: &str) -> bool {
        if self.state.lock().unwrap_or_else(|e| e.into_inner()).test_images.iter().any(|t| t == r) {
            return true;
        }
        let bare = r.strip_prefix("localhost/").unwrap_or(r);
        let (repo, tag) = bare.rsplit_once(':').unwrap_or((bare, ""));
        let repo = repo.rsplit('/').next().unwrap_or(repo);
        (repo.ends_with("-test") || repo.ends_with("-tests") || repo.ends_with("-unit-test")) && !repo.contains("integration") || tag.ends_with("-test")
    }

    /// The result of a build with a test target: counts, coverage and the test output; a failing test returns its output as the error.
    pub(super) fn finish_test_build(&self, reference: &str, target: &str, engine: &str, o: &super::Out) -> Result<String, String> {
        let text = Self::out_text(o);
        let (summary, clean, note) = crate::dockerlint::build_failure_summary(&text);
        let outcome = parse_outcome(&clean);
        if !o.ok() {
            let why = if o.timed_out { format!("the test build took longer than {} s and was stopped", super::BUILD_SECS) } else if summary.is_empty() { "the test stage failed".to_string() } else { format!("the test stage {summary}") };
            self.audit("test_build", &format!("{reference} failed"));
            return Err(format!("TEST BUILD FAILED (not for deploy; {engine}, target {target}): {why}.\n{}\nFailing test output (last lines):\n{}{}\nFix the code or the tests and build again; do not deploy.", outcome.summary(), tail_lines(&clean, 60), note.map(|n| format!("\n{n}")).unwrap_or_default()));
        }
        self.state.lock().unwrap_or_else(|e| e.into_inner()).test_images.push(reference.to_string());
        self.audit("test_build", &format!("{reference} passed"));
        let failed = outcome.counts.is_some_and(|c| c.1 > 0);
        Ok(format!("TEST BUILD (not for deploy): {reference} built with {engine}, target {target}. {} {}\n{}\nLast test output:\n{}\nThis is a test image: do NOT call deploy_load with it (it is refused). Record the counts and coverage on the board row, then build the runtime image with deploy_build without a target.{}", if failed { "The build succeeded but the output shows failing tests: treat it as FAILED." } else { "The tests ran inside the build and passed." }, if outcome.counts.is_some() || outcome.coverage.is_some() { "" } else { "(no counts found)" }, outcome.summary(), tail_lines(&clean, 40), note.map(|n| format!("\n{n}")).unwrap_or_default()))
    }

    // ------------------------------------------------------------------------------------------------------- the Job

    /// (Job name, its YAML, notes) for `deploy_run_job`.
    fn render_job(&self, args: &Value) -> Result<(String, String, Vec<String>, u64), String> {
        let image = arg(args, "image");
        if image.is_empty() || image.len() > 200 || image.chars().any(|c| c.is_whitespace() || c.is_control() || c == '"') {
            return Err("give `image`: the integration-test image (deploy_build it, then deploy_load and use the name it returns)".into());
        }
        if self.is_test_image(image) {
            return Err(format!("{image} is a unit-test image (built with a test target); build the integration-test image from tests/integration without a test target"));
        }
        let secs = args.get("timeout_seconds").and_then(Value::as_u64).unwrap_or(300).clamp(1, 1800);
        let base: String = image.rsplit('/').next().unwrap_or(image).split(':').next().unwrap_or("job").chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
        let name = if arg(args, "name").is_empty() { format!("{}-it", base.trim_matches('-').chars().take(40).collect::<String>()) } else { arg(args, "name").to_string() };
        if !object_name(&name) || name.len() > 52 {
            return Err(format!("`{name}` is not a usable Job name"));
        }
        let mut notes = Vec::new();
        let mut env: Vec<(String, String)> = Vec::new();
        if let Some(o) = args.get("env").and_then(Value::as_object) {
            for (k, v) in o {
                let val = match v {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    _ => return Err(format!("environment variable {k} must be a string, number or boolean")),
                };
                if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || val.len() > 1000 || val.chars().any(|c| c == '\n' || c == '\r' || c.is_control()) {
                    return Err(format!("environment variable `{k}` is not usable (name: letters, digits, _; value: one line)"));
                }
                env.push((k.clone(), val));
            }
        }
        if !env.iter().any(|(k, _)| k == "BASE_URL") {
            let o = self.kc().read(&["get", "services", "-n", &self.ns, "-o", "json"]);
            let svcs: Vec<(String, u64)> = if o.ok() {
                serde_json::from_str::<Value>(&o.stdout).ok().and_then(|j| j["items"].as_array().cloned()).unwrap_or_default().iter().filter(|s| s["spec"]["type"].as_str() != Some("ExternalName")).filter_map(|s| Some((s["metadata"]["name"].as_str()?.to_string(), s["spec"]["ports"].as_array()?.first()?["port"].as_u64()?))).collect()
            } else {
                vec![]
            };
            match svcs.as_slice() {
                [(n, p)] => {
                    env.push(("BASE_URL".into(), format!("http://{n}.{}.svc.cluster.local:{p}", self.ns)));
                    notes.push(format!("BASE_URL defaults to the only Service, {n}"));
                }
                [] => notes.push("no Service in the namespace, so BASE_URL is not set: pass env.BASE_URL if the tests need the app".into()),
                many => notes.push(format!("{} Services exist ({}), so BASE_URL is not guessed: pass env.BASE_URL", many.len(), many.iter().map(|s| s.0.clone()).collect::<Vec<_>>().join(", "))),
            }
        }
        let command: Vec<String> = match args.get("command") {
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
            Some(Value::String(s)) => s.split_whitespace().map(str::to_string).collect(),
            _ => vec![],
        };
        if command.iter().any(|c| c.chars().any(char::is_control)) {
            return Err("the command has control characters".into());
        }
        let ws: String = self.ws.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '-' }).collect();
        let mut y = format!("apiVersion: batch/v1\nkind: Job\nmetadata:\n  name: {name}\n  labels:\n    app.kubernetes.io/managed-by: iyakki\n    iyakki.dev/workspace: {ws}\n    iyakki.dev/job: integration-test\nspec:\n  backoffLimit: 0\n  activeDeadlineSeconds: {secs}\n  template:\n    metadata:\n      labels:\n        iyakki.dev/job: {name}\n    spec:\n      restartPolicy: Never\n      containers:\n      - name: tests\n        image: {image}\n        imagePullPolicy: IfNotPresent\n");
        if !command.is_empty() {
            y.push_str("        command:\n");
            for c in &command {
                y.push_str(&format!("        - {}\n", yq(c)));
            }
        }
        if !env.is_empty() {
            y.push_str("        env:\n");
            for (k, v) in &env {
                y.push_str(&format!("        - name: {k}\n          value: {}\n", yq(v)));
            }
        }
        y.push_str("        resources:\n          requests:\n            cpu: 100m\n            memory: 128Mi\n          limits:\n            cpu: \"1\"\n            memory: 512Mi\n        securityContext:\n          allowPrivilegeEscalation: false\n");
        if !args.get("allow_root").and_then(Value::as_bool).unwrap_or(false) {
            y.push_str("          runAsNonRoot: true\n");
        } else {
            notes.push("allow_root: the Job may run as root".into());
        }
        Ok((name, y, notes, secs))
    }

    pub(super) fn run_job_preview(&self, args: &Value) -> String {
        if let Some(e) = self.read_only() {
            return format!("this would be refused: {e}");
        }
        match self.render_job(args) {
            Ok((name, y, notes, secs)) => format!("Job {name} in namespace {} (timeout {secs} s, backoffLimit 0; deleted afterwards{}):\n{}\n{y}", self.ns, if args.get("keep").and_then(Value::as_bool).unwrap_or(false) { " NOT: keep=true" } else { "" }, notes.iter().map(|n| format!("note: {n}")).collect::<Vec<_>>().join("\n")),
            Err(e) => format!("this would be refused: {e}"),
        }
    }

    pub(super) fn run_job(&self, args: &Value) -> Result<String, String> {
        if let Some(e) = self.read_only() {
            return Err(e);
        }
        let (name, yaml, notes, secs) = self.render_job(args)?;
        let keep = args.get("keep").and_then(Value::as_bool).unwrap_or(false);
        let kc = self.kc();
        let dir = self.root.join(".iyakki/jobs");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let file = dir.join(format!("{name}.yaml"));
        std::fs::write(&file, &yaml).map_err(|e| e.to_string())?;
        // a Job of the same name from an earlier run goes first (a Job cannot be changed in place)
        let _ = kc.kubectl(&["delete", "job", &name, "-n", &self.ns, "--ignore-not-found", "--wait=true"], 10, 60);
        let applied = self.apply(&json!({"manifest_path": format!(".iyakki/jobs/{name}.yaml")}))?;
        self.audit("job", &format!("{name} started ({}) image {}", self.ns, arg(args, "image")));
        let started = Instant::now();
        let deadline = started + Duration::from_secs(secs + (secs / 10).clamp(1, 15));
        let (mut passed, mut failed) = (false, false);
        loop {
            let o = kc.read(&["get", "job", &name, "-n", &self.ns, "-o", "json"]);
            if o.ok() {
                if let Ok(j) = serde_json::from_str::<Value>(&o.stdout) {
                    let cond = |t: &str| j["status"]["conditions"].as_array().is_some_and(|c| c.iter().any(|x| x["type"] == t && x["status"] == "True"));
                    passed = j["status"]["succeeded"].as_u64().unwrap_or(0) > 0 || cond("Complete");
                    failed = j["status"]["failed"].as_u64().unwrap_or(0) > 0 || cond("Failed");
                }
            }
            if passed || failed || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_secs(3).min(deadline.saturating_duration_since(Instant::now())));
        }
        let took = started.elapsed().as_secs();
        let pod = self.job_pod(&name);
        let (podname, phase, waiting) = pod.clone().unwrap_or_default();
        let logs = if podname.is_empty() {
            String::new()
        } else {
            let l = kc.read(&["logs", &podname, "-n", &self.ns, "--tail", "300"]);
            if l.ok() { tail_chars(&l.stdout, 6000) } else { String::new() }
        };
        let outcome = parse_outcome(&logs);
        let mut report = String::new();
        if !passed && !failed {
            report.push_str(&format!("INTEGRATION TEST TIMED OUT after {took} s (Job {name}, image {}): the Job is still Active.\nPod {}: {}{}\n", arg(args, "image"), if podname.is_empty() { "(none was created)".to_string() } else { podname.clone() }, if phase.is_empty() { "unknown".to_string() } else { phase.clone() }, if waiting.is_empty() { String::new() } else { format!(" — {waiting}") }));
            if !podname.is_empty() {
                let ev = kc.read(&["get", "events", "-n", &self.ns, "--sort-by=.lastTimestamp"]);
                let lines: Vec<&str> = ev.stdout.lines().filter(|l| l.contains(&podname)).collect();
                let tail = lines[lines.len().saturating_sub(12)..].join("\n");
                report.push_str(&format!("{}\n", self.with_diagnosis(format!("{waiting}\n{tail}")).trim()));
            }
        } else if passed {
            report.push_str(&format!("INTEGRATION TEST PASSED in {took} s (Job {name}, image {}).\n", arg(args, "image")));
        } else {
            report.push_str(&format!("INTEGRATION TEST FAILED after {took} s (Job {name}, image {}).{}\n", arg(args, "image"), if waiting.is_empty() { String::new() } else { format!(" Pod: {waiting}") }));
            if waiting.contains("non-root") {
                report.push_str("The image runs as root but the Job requires non-root: set a numeric USER in the Dockerfile, or pass allow_root=true.\n");
            }
        }
        report.push_str(&format!("{}\n", outcome.summary()));
        if logs.trim().is_empty() {
            report.push_str("(the pod wrote no log)\n");
        } else {
            report.push_str(&format!("Pod logs (last {} chars):\n{}\n", logs.len().min(6000), logs.trim_end()));
        }
        for n in &notes {
            report.push_str(&format!("note: {n}\n"));
        }
        if applied.contains("warning:") {
            report.push_str("(the manifest check had warnings)\n");
        }
        if keep && (passed || failed) {
            report.push_str(&format!("The Job is kept: kubectl delete job {name} -n {} when you are done.\n", self.ns));
        } else {
            let d = kc.kubectl(&["delete", "job", &name, "-n", &self.ns, "--ignore-not-found", "--wait=false"], 10, 60);
            report.push_str(if d.ok() { "The Job was removed.\n" } else { "The Job could not be removed; delete it yourself.\n" });
        }
        self.audit("job", &format!("{name} {}", if passed { "passed" } else if failed { "failed" } else { "timed out" }));
        let exit_code = waiting.strip_prefix("exited ").and_then(|r| r.split_whitespace().next()).and_then(|n| n.parse().ok());
        self.state.lock().unwrap_or_else(|e| e.into_inner()).last_job = Some(JobRun { job: name.clone(), pod: podname.clone(), passed, failed, timed_out: !passed && !failed, exit_code, logs: logs.clone(), took_s: took, namespace: self.ns.clone() });
        report.push_str("Record the result on the board row (board_update).");
        if passed { Ok(report) } else { Err(cut(&report, 9000)) }
    }

    /// The pod of a Job: (name, phase, why it is waiting or how it ended).
    fn job_pod(&self, job: &str) -> Option<(String, String, String)> {
        let o = self.kc().read(&["get", "pods", "-n", &self.ns, "-l", &format!("job-name={job}"), "-o", "json"]);
        if !o.ok() {
            return None;
        }
        let j: Value = serde_json::from_str(&o.stdout).ok()?;
        let p = j["items"].as_array()?.first()?;
        let name = p["metadata"]["name"].as_str()?.to_string();
        let phase = p["status"]["phase"].as_str().unwrap_or("").to_string();
        let cs = &p["status"]["containerStatuses"][0]["state"];
        let why = if let Some(w) = cs["waiting"].as_object() {
            format!("waiting: {} {}", w.get("reason").and_then(Value::as_str).unwrap_or(""), first_line(w.get("message").and_then(Value::as_str).unwrap_or("")))
        } else if let Some(t) = cs["terminated"].as_object() {
            format!("exited {} ({})", t.get("exitCode").and_then(Value::as_i64).unwrap_or(-1), t.get("reason").and_then(Value::as_str).unwrap_or(""))
        } else {
            String::new()
        };
        Some((name, phase, why.trim().to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const JEST: &str = "PASS src/a.test.js\n----------|---------|----------|\nFile      | % Stmts | % Branch |\n----------|---------|----------|\nAll files |    87.5 |       80 |\n a.js     |    87.5 |       80 |\n\nTest Suites: 1 passed, 1 total\nTests:       1 failed, 5 passed, 6 total\n";
    const PYTEST: &str = "tests/test_a.py ....F\nName      Stmts   Miss  Cover\nTOTAL       120     10    91%\n======== 1 failed, 4 passed in 0.12s ========\n";
    const GO: &str = "ok  \texample.com/app\t0.01s\tcoverage: 78.2% of statements\nok  \texample.com/app/util\t0.02s\tcoverage: 60.0% of statements\nFAIL\texample.com/app/bad\t0.01s\n";
    const CARGO: &str = "running 3 tests\ntest result: ok. 3 passed; 0 failed; 0 ignored\ntest result: FAILED. 4 passed; 2 failed; 0 ignored\n";

    #[test]
    fn counts_and_coverage_are_read_from_jest_pytest_go_and_cargo_output() {
        assert_eq!(parse_outcome(JEST), Outcome { counts: Some((5, 1)), coverage: Some(87.5) });
        assert_eq!(parse_outcome(PYTEST), Outcome { counts: Some((4, 1)), coverage: Some(91.0) });
        let g = parse_outcome(GO);
        assert_eq!((g.counts, g.coverage), (Some((2, 1)), Some(69.1)));
        assert_eq!(parse_outcome(CARGO).counts, Some((7, 2)));
        assert_eq!(parse_outcome("Tests: 6 passed, 6 total").counts, Some((6, 0)));
        assert_eq!(parse_outcome("Total coverage: 90%").coverage, Some(90.0));
        assert_eq!(parse_outcome("5 passed in 0.10s").counts, Some((5, 0)));
        assert!(parse_outcome("nothing here").summary().contains("No pass/fail count"));
        assert_eq!(parse_outcome(JEST).summary(), "Tests: 5 passed, 1 failed. Coverage: 87.5%");
    }

    #[test]
    fn build_args_and_targets_are_validated() {
        assert_eq!(build_args(&json!({"build_args": {"NODE_ENV": "test", "N": 3}})).unwrap(), vec!["--build-arg", "N=3", "--build-arg", "NODE_ENV=test"]);
        assert!(build_args(&json!({"build_args": {"bad name": "x"}})).is_err());
        assert!(build_args(&json!({"build_args": {"A": "line\nbreak"}})).is_err());
        assert!(build_args(&json!({"build_args": {"A": [1]}})).is_err());
        assert!(is_test_target("test") && is_test_target("unit-test") && is_test_target("coverage") && !is_test_target("runtime") && !is_test_target(""));
    }
}
