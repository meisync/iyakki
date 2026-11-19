//! When an approval that waits is announced, reminded and reported expired: pure, on the clock that is passed in.

pub const ANNOUNCED: u8 = 1;
pub const HALF: u8 = 2;
pub const LATE: u8 = 4;
pub const EXPIRED: u8 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Announce,
    /// 50 % of the time to live has passed.
    Half,
    /// 90 %.
    Late,
    Expired,
}

impl Stage {
    pub fn bit(self) -> u8 {
        match self {
            Stage::Announce => ANNOUNCED,
            Stage::Half => HALF,
            Stage::Late => LATE,
            Stage::Expired => EXPIRED,
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Stage::Announce => "announce",
            Stage::Half => "r50",
            Stage::Late => "r90",
            Stage::Expired => "expired",
        }
    }
}

/// The stages that are due and not yet done (`mask`), in order. A request past its end only has its expiry to report.
pub fn due(created: u64, expires: u64, now: u64, mask: u8) -> Vec<Stage> {
    let mut v = Vec::new();
    if now >= expires {
        if mask & EXPIRED == 0 {
            v.push(Stage::Expired);
        }
        return v;
    }
    let span = expires.saturating_sub(created).max(1);
    let pct = now.saturating_sub(created).saturating_mul(100) / span;
    if mask & ANNOUNCED == 0 {
        v.push(Stage::Announce);
    }
    if pct >= 50 && mask & HALF == 0 {
        v.push(Stage::Half);
    }
    if pct >= 90 && mask & LATE == 0 {
        v.push(Stage::Late);
    }
    v
}
