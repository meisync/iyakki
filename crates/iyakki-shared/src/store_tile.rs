

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TestReport {
    pub passed: usize,
    pub failed: usize,
    pub lines: Vec<String>,
}

impl TestReport {
    pub fn ok(&self) -> bool {
        self.failed == 0 && self.passed > 0
    }

    pub fn text(&self) -> String {
        let mut o = self.lines.join("\n");
        o.push_str(&format!("\n{} passed, {} failed{}", self.passed, self.failed, if self.passed + self.failed == 0 { " (no tests ran: add tests/*.js that call T.test)" } else { "" }));
        o.trim_start().to_string()
    }
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----
use serde_json::Value;

/// A request the app made that needs the person: the shell shows it and answers with [`TileRuntime::resolve`].
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Request {
    pub id: u32,
    /// `host.openFile`, `host.saveFile`, `clipboard.read`, `clipboard.write`, `agent.ask`, `sdk.approve` (first use of `llm` or `speech`).
    pub kind: String,
    pub args: Value,
}
