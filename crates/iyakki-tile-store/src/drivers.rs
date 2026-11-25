//! The Drivers chip's read side as the STORE tile sees it (alpha.10 AP3, D-AP3-02): plain copies of the driver catalogue's enums and the
//! machine's rows, with the words the screen shows already in the answer (so no word is copied into this crate). The real answers come
//! from the core's driver read (`iyakki_core_api::drivers::Drivers`) through `CoreDrivers` in `crates/iyakki-apps/src/apps/store_port.rs`.

/// Works with the standard image, after the pack, needs the Mac image, a local build, or not supported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    Standard,
    AfterPack,
    NeedsMacImage,
    LocalBuild,
    NotSupported,
}

/// How a pack reaches the machine: built in, from a folder or stick, extracted from a file, or information only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    Builtin,
    Folder,
    Extract,
    Info,
}

/// The state of a driver on this machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Working,
    FirmwareMissing,
    NeedsReload,
    NoDriver,
}

/// One catalogue entry, as the screen reads it. `availability_word` is the words of `Availability` for the entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownInfo {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub why: String,
    pub supply: String,
    pub guide: String,
    pub license: String,
    pub availability: Availability,
    pub availability_word: String,
    pub delivery: Delivery,
}

/// One entry of the Drivers page: the entry, its state (and the state's words), the detail line and the devices found for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowInfo {
    pub known: KnownInfo,
    pub status: Status,
    pub status_word: String,
    pub detail: String,
    pub devices: Vec<String>,
}

/// An installed driver pack, as its record says it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackInfo {
    pub id: String,
    pub why: String,
    pub license: String,
    pub files: Vec<String>,
    pub source: String,
    pub cmdline: Vec<String>,
}

/// The machine's vendor and product name (the DMI names; empty when the machine has none).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MachineInfo {
    pub vendor: String,
    pub product: String,
}

/// The read side the Drivers chip draws from.
pub trait DriversView: Send + Sync {
    fn machine(&self) -> MachineInfo;
    /// The rows of the Drivers page (empty for a machine none of the catalogue entries applies to).
    fn rows(&self) -> Vec<RowInfo>;
    /// The installed driver packs.
    fn installed(&self) -> Vec<PackInfo>;
    /// Is the installed pack `id` set aside (turned off)?
    fn is_disabled(&self, id: &str) -> bool;
    /// The notice shown under "APPLE'S LICENCE" for an extracted pack.
    fn facetime_notice(&self) -> String;
}
