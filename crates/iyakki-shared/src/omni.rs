use crate::lemonade::Model;
use crate::lemonade::UsePlan;

/// Where a collection stands before it can be used.
#[derive(Clone, Debug, PartialEq)]
pub struct Review {
    pub collection: Model,
    pub plan: UsePlan,
    /// The collection or some part is not on the server (or the server lists no parts yet): install first.
    pub needs_install: bool,
}

impl Review {
    /// The parts that are missing, one per line: "pictures SD-Turbo: missing (2.6 GB)".
    pub fn missing_lines(&self) -> Vec<String> {
        if self.plan.parts.is_empty() {
            return vec![format!("{}: the parts are listed once it is downloaded ({})", self.collection.id, self.collection.size_text())];
        }
        self.plan.missing().iter().map(|p| format!("{}: {}", p.title(), p.state())).collect()
    }
}
