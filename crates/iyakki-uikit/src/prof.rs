//! A small per-thread stopwatch for finding where a frame goes (F-SHELL-64): `scope("name")` adds the time until it is dropped to the
//! name's total. Off unless [`enable`] was called on this thread (one flag read per call), so the shipped drawing pays almost nothing.
//! Times are inclusive: a `text` inside an `island` counts in both.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::time::Instant;

thread_local! {
    static ON: Cell<bool> = const { Cell::new(false) };
    static TOTALS: RefCell<BTreeMap<&'static str, (u64, u32)>> = const { RefCell::new(BTreeMap::new()) };
}

pub struct Scope(&'static str, Option<Instant>);

pub fn enable(on: bool) {
    ON.with(|c| c.set(on));
}

#[inline]
pub fn scope(name: &'static str) -> Scope {
    Scope(name, ON.with(|c| c.get()).then(Instant::now))
}

impl Drop for Scope {
    fn drop(&mut self) {
        if let Some(t) = self.1 {
            let ns = t.elapsed().as_nanos() as u64;
            TOTALS.with(|m| {
                let mut m = m.borrow_mut();
                let e = m.entry(self.0).or_insert((0, 0));
                e.0 += ns;
                e.1 += 1;
            });
        }
    }
}

/// The totals so far (name, nanoseconds, calls), biggest first, and start again from zero.
pub fn take() -> Vec<(&'static str, u64, u32)> {
    let mut v: Vec<_> = TOTALS.with(|m| std::mem::take(&mut *m.borrow_mut())).into_iter().map(|(k, (ns, n))| (k, ns, n)).collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.1));
    v
}
