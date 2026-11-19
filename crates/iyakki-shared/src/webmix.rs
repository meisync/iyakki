//! Types moved out of `iyakki_agent::webmix` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TapState {
    /// Connected, nothing arriving.
    Idle,
    /// Sound arriving and handed to the mixer.
    Carrying,
    /// Sound arriving and the mixer would not take it (it is dropped at the pace of real time).
    Refused,
}

impl TapState {
    pub fn code(self) -> u8 {
        match self {
            TapState::Idle => 0,
            TapState::Carrying => 1,
            TapState::Refused => 2,
        }
    }
    pub fn from_code(c: u8) -> TapState {
        match c {
            1 => TapState::Carrying,
            2 => TapState::Refused,
            _ => TapState::Idle,
        }
    }
}
