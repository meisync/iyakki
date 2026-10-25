//! Apple's board id for the recovery service, and the T2 Macs the hold-back applies to.
//!
//! Apple's recovery download asks for the board id (`Mac-7BA5B2DFE22DDD8C`) of the Mac the image is for. On every Intel Mac
//! the DMI `board_name` (`/sys/class/dmi/id/board_name`) IS that id, so [`resolve`] takes it from there for any Intel Mac,
//! T2 or not. The table below is only a cross-check, a name source and the "verified" flag: an entry is verified when its id
//! was seen on a real machine (the Mac mini 2018 by its owner, the MacBookPro14,1 from its own DMI). The other entries are
//! copied from the board lists that the OpenCore project's `macrecovery` tool and its SMBIOS data publish, from memory of
//! that list, and are marked unverified until someone runs them. A model that is not in the table is allowed: the person is
//! told that it is not on the verified list. A wrong id does no harm: Apple's service answers with an error and the job says so.

use crate::pack::glob;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Board {
    pub model: &'static str,
    pub board_id: &'static str,
    /// Seen on a real machine (or given by its owner), not just listed.
    pub verified: bool,
    pub name: &'static str,
}

pub const BOARDS: &[Board] = &[
    Board { model: "Macmini8,1", board_id: "Mac-7BA5B2DFE22DDD8C", verified: true, name: "Mac mini (2018)" },
    Board { model: "MacBookPro14,1", board_id: "Mac-B4831CEBD52A0C4C", verified: true, name: "MacBook Pro 13-inch, two ports (2017)" },
    Board { model: "MacBookPro15,1", board_id: "Mac-937A206F2EE63C01", verified: false, name: "MacBook Pro 15-inch (2018, 2019)" },
    Board { model: "MacBookPro15,2", board_id: "Mac-827FB448E656EC26", verified: false, name: "MacBook Pro 13-inch, four ports (2018, 2019)" },
    Board { model: "MacBookPro15,3", board_id: "Mac-1E7E29AD0135F9BC", verified: false, name: "MacBook Pro 15-inch Vega (2018, 2019)" },
    Board { model: "MacBookPro15,4", board_id: "Mac-53FDB3D8DB8CA971", verified: false, name: "MacBook Pro 13-inch, two ports (2019)" },
    Board { model: "MacBookPro16,1", board_id: "Mac-E1008331FDC96864", verified: false, name: "MacBook Pro 16-inch (2019)" },
    Board { model: "MacBookPro16,2", board_id: "Mac-5F9802EFE386AA28", verified: false, name: "MacBook Pro 13-inch, four ports (2020)" },
    Board { model: "MacBookPro16,3", board_id: "Mac-E7203C0F68AA0004", verified: false, name: "MacBook Pro 13-inch, two ports (2020)" },
    Board { model: "MacBookPro16,4", board_id: "Mac-A61BADE1FDAD7B05", verified: false, name: "MacBook Pro 16-inch (2019)" },
    Board { model: "MacBookAir8,1", board_id: "Mac-827FAC58A8FDFA22", verified: false, name: "MacBook Air (2018)" },
    Board { model: "MacBookAir8,2", board_id: "Mac-226CB3C6A851A671", verified: false, name: "MacBook Air (2019)" },
    Board { model: "MacBookAir9,1", board_id: "Mac-0CFF9C7C2B63DF8D", verified: false, name: "MacBook Air (2020)" },
    Board { model: "iMac20,1", board_id: "Mac-CFF7D910A743CAAF", verified: false, name: "iMac 27-inch (2020)" },
    Board { model: "iMac20,2", board_id: "Mac-AF89B6D9451A490B", verified: false, name: "iMac 27-inch (2020)" },
    Board { model: "iMacPro1,1", board_id: "Mac-7BA5B2D9E42DDD94", verified: false, name: "iMac Pro (2017)" },
    Board { model: "MacPro7,1", board_id: "Mac-27AD2F918AE68F61", verified: false, name: "Mac Pro (2019)" },
];

/// The DMI product names of the Macs with the T2 chip (the same list as the `mac-t2` entry of the known drivers).
pub const T2_MODELS: &[&str] = &["MacBookPro15,*", "MacBookPro16,*", "MacBookAir8,*", "MacBookAir9,*", "iMac20,*", "iMacPro1,1", "Macmini8,1", "MacPro7,1"];

/// T2 Macs whose Bluetooth is the BCM4364 on a UART (asks for `brcm/BCM.hcd`); the others use BCM4377 on PCIe (`brcmbt4377*`).
/// The Mac mini 2018 is verified; the MacBook Pro models are from the notes in docs/MAC-HARDWARE.md.
pub fn uart_bluetooth(dmi_product: &str) -> bool {
    matches!(dmi_product.trim(), "Macmini8,1" | "MacBookPro15,1" | "MacBookPro15,2" | "MacBookPro15,3" | "MacBookPro16,1" | "MacBookPro16,2")
}

pub fn board_for(dmi_product: &str) -> Option<&'static Board> {
    let p = dmi_product.trim();
    BOARDS.iter().find(|b| b.model.eq_ignore_ascii_case(p))
}

/// The board id the DMI `board_name` carries (`Mac-B4831CEBD52A0C4C`), when it has Apple's shape.
pub fn board_id_from_dmi(board_name: &str) -> Option<String> {
    let b = board_name.trim();
    valid_board_id(b).then(|| b.to_string())
}

/// What is known about the board of this Mac.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    /// The id to ask Apple for: the DMI one when it is well formed, else the table's.
    pub board_id: Option<String>,
    /// The table entry for the model, if any.
    pub table: Option<&'static Board>,
    /// The id is the one a real machine gave (the table entry is verified and the id equals it).
    pub verified: bool,
    pub t2: bool,
    /// The id came from the DMI (not from the table).
    pub from_dmi: bool,
    /// The table and the DMI disagree (the DMI wins).
    pub mismatch: Option<String>,
}

pub fn resolve(dmi_product: &str, dmi_board_name: &str) -> Resolved {
    let table = board_for(dmi_product);
    let dmi = board_id_from_dmi(dmi_board_name);
    let mismatch = match (&dmi, table) {
        (Some(d), Some(t)) if d != t.board_id => Some(format!("IYAKKI's list says {} for {}, this Mac's firmware says {d}: the firmware's id is used", t.board_id, t.model)),
        _ => None,
    };
    let board_id = dmi.clone().or_else(|| table.map(|t| t.board_id.to_string()));
    let verified = matches!((&board_id, table), (Some(id), Some(t)) if t.verified && id == t.board_id);
    Resolved { board_id, table, verified, t2: is_t2(dmi_product), from_dmi: dmi.is_some(), mismatch }
}

pub fn is_t2(dmi_product: &str) -> bool {
    let p = dmi_product.trim();
    T2_MODELS.iter().any(|m| glob(m, p))
}

/// A board id as Apple writes it: `Mac-` and 16 hex digits.
pub fn valid_board_id(b: &str) -> bool {
    b.len() == 20 && b.starts_with("Mac-") && b[4..].chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mac_mini_2018_maps_to_the_board_id_the_owner_gave_and_is_marked_verified() {
        let b = board_for("Macmini8,1").unwrap();
        assert_eq!(b.board_id, "Mac-7BA5B2DFE22DDD8C");
        assert!(b.verified);
        assert_eq!(board_for(" macmini8,1\n").unwrap().board_id, b.board_id, "case and spaces do not matter");
    }

    #[test]
    fn every_entry_has_a_well_formed_unique_board_id_and_only_the_two_seen_on_real_machines_are_verified() {
        let mut ids = std::collections::BTreeSet::new();
        for b in BOARDS {
            assert!(valid_board_id(b.board_id), "{}", b.model);
            assert!(ids.insert(b.board_id), "duplicate id {}", b.board_id);
        }
        let verified: Vec<&str> = BOARDS.iter().filter(|b| b.verified).map(|b| b.model).collect();
        assert_eq!(verified, vec!["Macmini8,1", "MacBookPro14,1"]);
        assert!(BOARDS.iter().filter(|b| !is_t2(b.model)).all(|b| b.model == "MacBookPro14,1"), "the only non-T2 entry is the 2017 13-inch");
    }

    #[test]
    fn the_dmi_board_name_is_the_board_id_on_any_intel_mac_and_the_table_is_a_cross_check() {
        // the real MacBookPro14,1 (2017, 13 inch, not a T2 Mac)
        let r = resolve("MacBookPro14,1", "Mac-B4831CEBD52A0C4C\n");
        assert_eq!((r.board_id.as_deref(), r.verified, r.t2, r.from_dmi), (Some("Mac-B4831CEBD52A0C4C"), true, false, true));
        assert!(r.mismatch.is_none());
        // a model that is not in the table is allowed, and not verified
        let r = resolve("iMac18,3", "Mac-77EB7D7DAF985301");
        assert_eq!((r.board_id.as_deref(), r.verified, r.table.is_none(), r.t2), (Some("Mac-77EB7D7DAF985301"), false, true, false));
        // a T2 Mac keeps working from the table when the DMI board name is missing or malformed
        for bad in ["", "Mac-7BA5B2DFE22DDD8", "mac-7ba5b2dfe22ddd8c", "Apple board"] {
            let r = resolve("Macmini8,1", bad);
            assert_eq!((r.board_id.as_deref(), r.verified, r.t2, r.from_dmi), (Some("Mac-7BA5B2DFE22DDD8C"), true, true, false), "{bad}");
        }
        // the DMI wins over a table that disagrees, and says so; the id is then not "verified"
        let r = resolve("Macmini8,1", "Mac-0000000000000000");
        assert_eq!(r.board_id.as_deref(), Some("Mac-0000000000000000"));
        assert!(!r.verified && r.mismatch.as_deref().unwrap().contains("Mac-7BA5B2DFE22DDD8C"));
        // nothing known at all
        let r = resolve("Macmini7,1", "");
        assert_eq!((r.board_id, r.table.is_none(), r.t2), (None, true, false));
        assert_eq!(board_id_from_dmi(" Mac-B4831CEBD52A0C4C "), Some("Mac-B4831CEBD52A0C4C".into()));
        assert_eq!(board_id_from_dmi("Mac-B4831CEBD52A0C4c"), None);
    }

    #[test]
    fn the_uart_bluetooth_macs_are_the_bcm4364_ones() {
        assert!(uart_bluetooth("Macmini8,1") && uart_bluetooth(" MacBookPro15,2\n"));
        assert!(!uart_bluetooth("MacBookPro15,4") && !uart_bluetooth("MacBookAir9,1") && !uart_bluetooth(""));
    }

    #[test]
    fn unknown_models_and_non_t2_macs_have_no_board_and_t2_detection_uses_the_dmi_name() {
        assert!(board_for("Macmini7,1").is_none() && board_for("").is_none() && board_for("ThinkPad").is_none());
        assert!(is_t2("MacBookPro16,1") && is_t2("Macmini8,1") && is_t2("iMacPro1,1"));
        assert!(!is_t2("Macmini7,1") && !is_t2("MacBookPro14,2") && !is_t2("iMac19,1") && !is_t2("") && !is_t2("PowerEdge R640"));
        assert!(!valid_board_id("mac-7ba5b2dfe22ddd8c") && !valid_board_id("Mac-7BA5B2DFE22DDD8") && !valid_board_id("Mac-7BA5B2DFE22DDD8G"));
    }
}
