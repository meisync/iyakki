// SPDX-License-Identifier: MIT OR Apache-2.0
//! One thing the menu can start: a configured entry or a discovered system.

use crate::choose::HoldKey;
use crate::discover::Target;
use alloc::string::String;

/// Why an item is shown greyed out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disabled {
    /// The loader is not signed and Secure Boot is on.
    UnsignedSecureBoot,
}

impl Disabled {
    /// Text for the menu.
    pub fn text(&self) -> &'static str {
        match self {
            Disabled::UnsignedSecureBoot => "not signed, Secure Boot is on",
        }
    }
}

/// What starting the item does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Start the Linux entry at this index of `Config::entries`.
    Linux(usize),
    /// Chain-load another system.
    Chain(Target),
    /// Restart into the firmware's setup screen.
    Firmware,
}

/// A bootable item with the fields the choice and the menu need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Stable id: the entry name, or the discovery id.
    pub id: String,
    /// Text shown in the menu.
    pub title: String,
    /// Not listed in the menu (still reachable by default, one-shot or hold key).
    pub hidden: bool,
    /// Lower comes first.
    pub order: i32,
    /// Marked as the default.
    pub default: bool,
    /// Key that selects it when held at start.
    pub hold: Option<HoldKey>,
    /// Listed on the second level ("More options").
    pub more: bool,
    /// Greyed out and not startable.
    pub disabled: Option<Disabled>,
    /// What to do.
    pub action: Action,
}

impl Item {
    /// Can this item be started?
    pub fn enabled(&self) -> bool {
        self.disabled.is_none()
    }
}
