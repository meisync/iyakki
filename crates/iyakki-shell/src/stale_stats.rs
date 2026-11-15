//! Who asked for a frame (KI-092). Every place that makes the shell's picture stale goes through [`mark_stale!`], and the backend
//! counts a timed (safety net) redraw under its own name, so the 10 second frame line can say which one is drawing an idle screen.

use std::cell::RefCell;
use std::collections::BTreeMap;

thread_local! {
    static COUNTS: RefCell<BTreeMap<String, u32>> = const { RefCell::new(BTreeMap::new()) };
}

/// Count one request under a name (a source line, or `timed` for the backend's safety net).
pub fn note(reason: &str) {
    COUNTS.with(|c| *c.borrow_mut().entry(reason.to_string()).or_insert(0) += 1);
}

/// Count a request made by `mark_stale!` at a source line.
pub fn note_at(file: &str, line: u32) {
    let short = file.rsplit('/').next().unwrap_or(file);
    note(&format!("{short}:{line}"));
}

/// The counts since the last call as `a=3, b=1` (largest first); empty when nothing was counted. Resets the counts.
pub fn take_table() -> String {
    COUNTS.with(|c| {
        let mut v: Vec<(String, u32)> = std::mem::take(&mut *c.borrow_mut()).into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v.iter().map(|(k, n)| format!("{k}={n}")).collect::<Vec<_>>().join(", ")
    })
}

/// `mark_stale!(self)`: the same as `self.stale = true`, and the source line is counted.
macro_rules! mark_stale {
    ($s:expr) => {{
        $s.stale = true;
        $crate::stale_stats::note_at(file!(), line!());
    }};
}
pub(crate) use mark_stale;
