//! Settings -> Radio: what the radio does when a live station's connection ends or fails while it plays (F-RADIO-17).
//! The same two choices are on the page and in the chat tools `radio_settings_status` / `radio_settings_set`.

use serde::{Deserialize, Serialize};

/// Reconnect tries of the same station before the radio moves on (when nothing is set).
pub const DEFAULT_RETRIES: u32 = 2;
/// The most tries a person may ask for.
pub const MAX_RETRIES: u32 = 5;

/// Stored with the machine settings. Both fields read "the default" when absent, so old files load unchanged.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RadioRecover {
    /// `None` is the default, [`DEFAULT_RETRIES`].
    pub retries: Option<u32>,
    /// Off by default: when the retries fail the radio goes on to the next station of the list.
    pub no_auto_switch: bool,
}

impl RadioRecover {
    pub fn retries(&self) -> u32 {
        self.retries.unwrap_or(DEFAULT_RETRIES).min(MAX_RETRIES)
    }
    pub fn auto_switch(&self) -> bool {
        !self.no_auto_switch
    }
    pub fn validate(&self) -> Result<(), String> {
        match self.retries {
            Some(n) if n > MAX_RETRIES => Err(format!("retries is 0 to {MAX_RETRIES}")),
            _ => Ok(()),
        }
    }
    /// One sentence for the page and the chat tool.
    pub fn describe(&self) -> String {
        format!(
            "When a live station drops, the radio reconnects {} time{} and then {}.",
            self.retries(),
            if self.retries() == 1 { "" } else { "s" },
            if self.auto_switch() { "goes on to the next station of the list (one pass at most)" } else { "stops (switching to the next station is off)" }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_two_retries_and_switching_on_and_old_files_load() {
        let r = RadioRecover::default();
        assert_eq!((r.retries(), r.auto_switch()), (2, true));
        let back: RadioRecover = serde_json::from_str("{}").unwrap();
        assert_eq!(back, r);
        assert!(RadioRecover { retries: Some(6), ..Default::default() }.validate().is_err());
        assert!(RadioRecover { retries: Some(0), no_auto_switch: true }.describe().contains("0 times") );
    }
}
