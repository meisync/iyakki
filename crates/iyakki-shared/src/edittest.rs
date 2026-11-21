use std::path::Path;
use std::path::PathBuf;

/// The longest a test run may take.
pub const MAX_SECONDS: u64 = 600;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TestPlan {
    /// `cargo test` or `pytest`.
    pub label: String,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

pub fn up_to(start: &Path, marker: &[&str]) -> Option<PathBuf> {
    let mut d = start.parent()?.to_path_buf();
    loop {
        if marker.iter().any(|m| d.join(m).is_file()) {
            return Some(d);
        }
        if !d.pop() {
            return None;
        }
    }
}

/// What to run for this file, or why nothing can be run.
pub fn plan(file: &Path) -> Result<TestPlan, String> {
    let ext = file.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match ext.as_str() {
        "rs" => {
            let cwd = up_to(file, &["Cargo.toml"]).ok_or("This Rust file is not inside a Cargo project (no Cargo.toml above it).")?;
            let mut args = vec!["test".to_string(), "--color".into(), "never".into()];
            // a file under tests/ is one test binary: run just that one
            if let Ok(rel) = file.strip_prefix(&cwd) {
                let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
                if parts.len() == 2 && parts[0] == "tests" {
                    args.push("--test".into());
                    args.push(parts[1].trim_end_matches(".rs").to_string());
                }
            }
            Ok(TestPlan { label: "cargo test".into(), program: "cargo".into(), args, cwd })
        }
        "py" => {
            let is_test = name.starts_with("test_") || name.ends_with("_test.py") || name == "conftest.py";
            if !is_test {
                return Err("Run tests works on a Python file named test_*.py or *_test.py.".into());
            }
            let cwd = up_to(file, &["pyproject.toml", "pytest.ini", "setup.cfg", "tox.ini"]).unwrap_or_else(|| file.parent().map(Path::to_path_buf).unwrap_or_default());
            Ok(TestPlan { label: "pytest".into(), program: "python3".into(), args: vec!["-m".into(), "pytest".into(), "-v".into(), "--color=no".into(), "-p".into(), "no:cacheprovider".into(), file.display().to_string()], cwd })
        }
        _ => Err("Tests can be run from a .rs file in a Cargo project or a test_*.py file.".into()),
    }
}

/// The account of one test run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestRun {
    pub ok: bool,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub failures: Vec<String>,
    /// The last lines of the output (text).
    pub tail: String,
    pub stopped: bool,
    pub timed_out: bool,
    pub exit: Option<i32>,
}

impl TestRun {
    /// The text shown under the editor.
    pub fn text(&self, label: &str) -> String {
        let mut s = if self.stopped {
            format!("{label}: stopped.")
        } else if self.timed_out {
            format!("{label}: ran out of time and was ended.")
        } else if self.passed + self.failed + self.skipped == 0 {
            format!("{label}: no test results found (exit {}).", self.exit.map_or("?".to_string(), |c| c.to_string()))
        } else {
            format!("{label}: {} passed, {} failed, {} skipped.", self.passed, self.failed, self.skipped)
        };
        for f in self.failures.iter().take(10) {
            s.push_str(&format!("\n  FAILED {f}"));
        }
        if self.failures.len() > 10 {
            s.push_str(&format!("\n  ... and {} more", self.failures.len() - 10));
        }
        if !self.ok && !self.tail.is_empty() {
            s.push_str(&format!("\n{}", self.tail));
        }
        s
    }
}
