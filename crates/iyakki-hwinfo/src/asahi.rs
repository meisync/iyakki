//! Apple Silicon identified from the device tree (F-APPLE-03). A Mac with an M-series chip has no DMI; Asahi Linux describes the machine
//! in the device tree: `/proc/device-tree/compatible` lists NUL-separated strings, `apple,j274` (the board) before `apple,t8103` (the
//! chip) before `apple,arm-platform`. This is pure text work: the table says which chips Asahi can run (M1 and M2 families) and which
//! it cannot yet (M3 and later), so the hardware report, the installer and the driver packs can say so in plain words instead of guessing.

/// What Asahi Linux can do with the chip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    Supported,
    /// Said in words: the chip is newer than what Asahi supports.
    NotYet,
}

/// An Apple Silicon computer as the device tree names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppleSilicon {
    /// `j274`: the board.
    pub board: String,
    /// `t8103`: the chip.
    pub chip_id: String,
    /// `M1`, `M2 Pro`...; empty for a chip id not in the table.
    pub chip: &'static str,
    /// `Mac mini (M1, 2020)`; empty for a board not in the table.
    pub model: &'static str,
    pub support: Support,
}

const CHIPS: [(&str, &str, Support); 18] = [
    ("t8103", "M1", Support::Supported),
    ("t6000", "M1 Pro", Support::Supported),
    ("t6001", "M1 Max", Support::Supported),
    ("t6002", "M1 Ultra", Support::Supported),
    ("t8112", "M2", Support::Supported),
    ("t6020", "M2 Pro", Support::Supported),
    ("t6021", "M2 Max", Support::Supported),
    ("t6022", "M2 Ultra", Support::Supported),
    ("t8122", "M3", Support::NotYet),
    ("t6030", "M3 Pro", Support::NotYet),
    ("t6031", "M3 Max", Support::NotYet),
    ("t6034", "M3 Max", Support::NotYet),
    ("t6035", "M3 Ultra", Support::NotYet),
    ("t8132", "M4", Support::NotYet),
    ("t6040", "M4 Pro", Support::NotYet),
    ("t6041", "M4 Max", Support::NotYet),
    ("t8140", "M5", Support::NotYet),
    ("t8101", "A14", Support::NotYet),
];

const BOARDS: [(&str, &str); 24] = [
    ("j274", "Mac mini (M1, 2020)"),
    ("j293", "MacBook Pro (13-inch, M1, 2020)"),
    ("j313", "MacBook Air (M1, 2020)"),
    ("j456", "iMac (24-inch, M1, 2021)"),
    ("j457", "iMac (24-inch, M1, 2021)"),
    ("j314s", "MacBook Pro (14-inch, M1 Pro, 2021)"),
    ("j314c", "MacBook Pro (14-inch, M1 Max, 2021)"),
    ("j316s", "MacBook Pro (16-inch, M1 Pro, 2021)"),
    ("j316c", "MacBook Pro (16-inch, M1 Max, 2021)"),
    ("j375c", "Mac Studio (M1 Max, 2022)"),
    ("j375d", "Mac Studio (M1 Ultra, 2022)"),
    ("j413", "MacBook Air (M2, 2022)"),
    ("j493", "MacBook Pro (13-inch, M2, 2022)"),
    ("j415", "MacBook Air (15-inch, M2, 2023)"),
    ("j473", "Mac mini (M2, 2023)"),
    ("j474s", "Mac mini (M2 Pro, 2023)"),
    ("j414s", "MacBook Pro (14-inch, M2 Pro, 2023)"),
    ("j414c", "MacBook Pro (14-inch, M2 Max, 2023)"),
    ("j416s", "MacBook Pro (16-inch, M2 Pro, 2023)"),
    ("j416c", "MacBook Pro (16-inch, M2 Max, 2023)"),
    ("j475c", "Mac Studio (M2 Max, 2023)"),
    ("j475d", "Mac Studio (M2 Ultra, 2023)"),
    ("j180d", "Mac Pro (M2 Ultra, 2023)"),
    ("j433", "MacBook Air (13-inch, M3, 2024)"),
];

/// Read the `compatible` property (bytes: NUL-separated strings). `None` when it names no Apple board and chip.
pub fn from_compatible(bytes: &[u8]) -> Option<AppleSilicon> {
    let list: Vec<&str> = bytes.split(|b| *b == 0).filter_map(|s| std::str::from_utf8(s).ok()).filter(|s| !s.is_empty()).collect();
    let apple = |s: &&str| s.strip_prefix("apple,").map(str::to_string);
    let ids: Vec<String> = list.iter().filter_map(apple).collect();
    let chip_id = ids.iter().find(|i| i.len() >= 5 && i.starts_with('t') && i[1..].bytes().all(|b| b.is_ascii_digit())).cloned()?;
    let board = ids.iter().find(|i| i.starts_with('j') && i[1..].bytes().next().is_some_and(|b| b.is_ascii_digit()) && i.bytes().all(|b| b.is_ascii_alphanumeric())).cloned()?;
    let (chip, support) = CHIPS.iter().find(|(id, _, _)| *id == chip_id).map_or(("", Support::NotYet), |(_, c, s)| (*c, *s));
    let model = BOARDS.iter().find(|(b, _)| *b == board).map_or("", |(_, m)| *m);
    Some(AppleSilicon { board, chip_id, chip, model, support })
}

impl AppleSilicon {
    /// One line for the hardware report.
    pub fn line(&self) -> String {
        let name = if self.model.is_empty() { format!("an Apple Silicon computer (board {})", self.board) } else { self.model.to_string() };
        let chip = if self.chip.is_empty() { format!("chip {}", self.chip_id) } else { format!("chip {} ({})", self.chip, self.chip_id) };
        let sup = match (self.support, self.chip.is_empty()) {
            (Support::Supported, _) => "Asahi Linux supports this chip family",
            (Support::NotYet, true) => "this chip is not in IYAKKI's table; Asahi Linux may not support it",
            (Support::NotYet, false) => "Asahi Linux does not support this chip yet: use the macOS bridge (route 2) or the Lima machine (route 3) instead",
        };
        format!("{name}; {chip}; {sup}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compat(items: &[&str]) -> Vec<u8> {
        items.iter().flat_map(|s| s.bytes().chain([0])).collect()
    }

    #[test]
    fn an_m1_mac_mini_is_named_and_supported() {
        let a = from_compatible(&compat(&["apple,j274", "apple,t8103", "apple,arm-platform"])).unwrap();
        assert_eq!((a.board.as_str(), a.chip, a.model, a.support), ("j274", "M1", "Mac mini (M1, 2020)", Support::Supported));
        assert_eq!(a.line(), "Mac mini (M1, 2020); chip M1 (t8103); Asahi Linux supports this chip family");
    }

    #[test]
    fn m2_pro_and_max_are_supported_and_m3_and_later_say_what_to_use_instead() {
        assert_eq!(from_compatible(&compat(&["apple,j474s", "apple,t6020", "apple,arm-platform"])).unwrap().support, Support::Supported);
        let m3 = from_compatible(&compat(&["apple,j433", "apple,t8122", "apple,arm-platform"])).unwrap();
        assert_eq!((m3.chip, m3.support), ("M3", Support::NotYet));
        assert!(m3.line().contains("does not support this chip yet") && m3.line().contains("route 2") && m3.line().contains("route 3"));
        for chip in ["apple,t6031", "apple,t8132", "apple,t6041", "apple,t8140"] {
            assert_eq!(from_compatible(&compat(&["apple,j999", chip])).unwrap().support, Support::NotYet, "{chip}");
        }
    }

    #[test]
    fn an_unknown_board_or_chip_is_said_by_its_id_and_never_claimed_supported() {
        let a = from_compatible(&compat(&["apple,j999", "apple,t8103"])).unwrap();
        assert_eq!(a.model, "");
        assert!(a.line().starts_with("an Apple Silicon computer (board j999); chip M1 (t8103)"));
        let b = from_compatible(&compat(&["apple,j274", "apple,t9999"])).unwrap();
        assert_eq!((b.chip, b.support), ("", Support::NotYet));
        assert!(b.line().contains("chip t9999") && b.line().contains("not in IYAKKI's table"));
    }

    #[test]
    fn anything_that_is_not_an_apple_board_with_a_chip_is_none() {
        assert!(from_compatible(b"").is_none());
        assert!(from_compatible(&compat(&["raspberrypi,5-model-b", "brcm,bcm2712"])).is_none());
        assert!(from_compatible(&compat(&["apple,arm-platform"])).is_none(), "no board and no chip");
        assert!(from_compatible(&compat(&["apple,j274"])).is_none(), "a board without its chip is not enough");
        assert!(from_compatible(&[0xff, 0xfe, 0, b'a']).is_none());
    }
}
