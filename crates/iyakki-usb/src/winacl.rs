//! The owner and access-list check of the plan file on Windows (F-INST-12). The Win32 side only asks for the file's security
//! descriptor as an SDDL string and for the caller's SID (`windows_real::file_security`); everything that decides is plain text parsing
//! here, so it runs and is tested on any system with SDDL strings written down from real Windows files.
//!
//! The rule: the file's owner is the person who asked for the install (or the Administrators group, which an elevated person may own
//! files as), and no ALLOW entry that applies to the file itself gives write, append, delete, change-permissions or take-ownership
//! rights to anybody else than that person, SYSTEM, the Administrators group or the file's creator-owner placeholder.

const SYSTEM: &str = "S-1-5-18";
const ADMINS: &str = "S-1-5-32-544";
const CREATOR_OWNER: &str = "S-1-3-0";
const OWNER_RIGHTS: &str = "S-1-3-4";

/// The rights that let somebody change the file or its permissions: FILE_WRITE_DATA, FILE_APPEND_DATA, FILE_WRITE_EA, FILE_DELETE_CHILD,
/// FILE_WRITE_ATTRIBUTES, DELETE, WRITE_DAC, WRITE_OWNER, GENERIC_WRITE, GENERIC_ALL.
const WRITE_MASK: u32 = 0x2 | 0x4 | 0x10 | 0x40 | 0x100 | 0x1_0000 | 0x4_0000 | 0x8_0000 | 0x4000_0000 | 0x1000_0000;

/// One allow entry of the access list: who, and which rights (a bit mask).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ace {
    pub trustee: String,
    pub mask: u32,
}

/// What the check needs from a file: its owner and the allow entries that apply to the file itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Security {
    pub owner: String,
    pub allows: Vec<Ace>,
    /// The list is missing (`D:` absent): everyone has full access on Windows, so this is never trusted.
    pub no_dacl: bool,
}

fn sid_of(alias: &str, user: &str) -> String {
    match alias {
        "SY" => SYSTEM.into(),
        "BA" => ADMINS.into(),
        "CO" => CREATOR_OWNER.into(),
        "OW" => OWNER_RIGHTS.into(),
        "WD" => "S-1-1-0".into(),
        "AU" => "S-1-5-11".into(),
        "BU" => "S-1-5-32-545".into(),
        "IU" => "S-1-5-4".into(),
        "AN" => "S-1-5-7".into(),
        "PS" => "S-1-5-10".into(),
        "LS" | "NS" => alias.into(),
        "" => user.into(),
        other => other.to_ascii_uppercase(),
    }
}

fn right_bits(word: &str) -> Option<u32> {
    let w = word.trim();
    if let Some(h) = w.strip_prefix("0x").or_else(|| w.strip_prefix("0X")) {
        return u32::from_str_radix(h, 16).ok();
    }
    if w.chars().all(|c| c.is_ascii_digit()) && !w.is_empty() {
        return w.parse().ok();
    }
    let mut bits = 0u32;
    let b = w.as_bytes();
    if b.len() % 2 != 0 {
        return None;
    }
    for pair in b.chunks(2) {
        bits |= match std::str::from_utf8(pair).ok()? {
            "GA" => 0x1000_0000,
            "GX" => 0x2000_0000,
            "GW" => 0x4000_0000,
            "GR" => 0x8000_0000,
            "FA" => 0x001f_01ff,
            "FR" => 0x0012_0089,
            "FW" => 0x0012_0116,
            "FX" => 0x0012_00a0,
            "CC" => 0x1,
            "DC" => 0x2,
            "LC" => 0x4,
            "SW" => 0x8,
            "RP" => 0x10,
            "WP" => 0x20,
            "DT" => 0x40,
            "LO" => 0x80,
            "CR" => 0x100,
            "SD" => 0x1_0000,
            "RC" => 0x2_0000,
            "WD" => 0x4_0000,
            "WO" => 0x8_0000,
            _ => return None,
        };
    }
    Some(bits)
}

/// Read an SDDL string (`O:BAG:SYD:PAI(A;;FA;;;BA)(A;;FR;;;S-1-5-21-...)`). `user_sid` stands in for an empty trustee. Anything not
/// understood is an error, never a guess: the check then refuses the file.
pub fn parse_sddl(sddl: &str, user_sid: &str) -> Result<Security, String> {
    let s = sddl.trim();
    let find = |tag: &str| -> Option<usize> { s.find(tag) };
    let o = find("O:").ok_or("the file's owner is not in its security descriptor")?;
    let rest = &s[o + 2..];
    let owner_end = rest.find("G:").or_else(|| rest.find("D:")).or_else(|| rest.find("S:")).unwrap_or(rest.len());
    let owner = sid_of(rest[..owner_end].trim(), user_sid);
    if owner.is_empty() {
        return Err("the file's owner is empty".into());
    }
    let Some(d) = find("D:") else {
        return Ok(Security { owner, allows: Vec::new(), no_dacl: true });
    };
    let dacl_all = &s[d + 2..];
    let dacl = dacl_all.split("S:").next().unwrap_or("");
    let mut allows = Vec::new();
    let mut open = dacl;
    while let Some(i) = open.find('(') {
        let j = open[i..].find(')').ok_or("an access entry is not closed")? + i;
        let f: Vec<&str> = open[i + 1..j].split(';').collect();
        open = &open[j + 1..];
        if f.len() < 6 {
            return Err(format!("an access entry has {} fields, expected six", f.len()));
        }
        if f[0] != "A" && f[0] != "D" && f[0] != "OA" && f[0] != "OD" && f[0] != "XA" && f[0] != "XD" {
            continue; // audit, label and similar entries grant nothing
        }
        if f[0] != "A" && f[0] != "OA" && f[0] != "XA" {
            continue; // a deny entry only takes rights away
        }
        if f[1].contains("IO") {
            continue; // inherit-only: it is for children, not for this file
        }
        let mask = right_bits(f[2]).ok_or_else(|| format!("access rights `{}` are not understood", f[2]))?;
        allows.push(Ace { trustee: sid_of(f[5].trim(), user_sid), mask });
    }
    Ok(Security { owner, allows, no_dacl: false })
}

/// The rule. `user_sid` is the caller's SID; `elevated` says the process runs with administrator rights.
pub fn plan_file_acl_ok(sec: &Security, user_sid: &str, elevated: bool) -> Result<(), String> {
    let user = user_sid.to_ascii_uppercase();
    if user.is_empty() {
        return Err("the caller's account is not known, so the plan file cannot be trusted; refused".into());
    }
    if sec.no_dacl {
        return Err("the plan file has no access list, so everyone may change it; refused".into());
    }
    let owner = sec.owner.to_ascii_uppercase();
    if !(owner == user || (elevated && owner == ADMINS)) {
        return Err("the plan file must be owned by the person who asked for the install; refused".into());
    }
    for a in &sec.allows {
        let t = a.trustee.to_ascii_uppercase();
        let trusted = t == user || t == SYSTEM || t == ADMINS || t == CREATOR_OWNER || t == OWNER_RIGHTS;
        if !trusted && a.mask & WRITE_MASK != 0 {
            return Err(format!("the plan file can be changed by {}, who is not the person who asked for the install; refused", a.trustee));
        }
    }
    Ok(())
}

/// Windows only: ask Win32 for the file's security and the caller's SID, then apply the rule.
#[cfg(windows)]
pub fn check_plan_file(file: &std::path::Path) -> Result<(), String> {
    let (sddl, user) = crate::windows_real::file_security(file)?;
    let sec = parse_sddl(&sddl, &user)?;
    plan_file_acl_ok(&sec, &user, crate::posix::is_root())
}

#[cfg(test)]
mod tests {
    use super::*;
    const ME: &str = "S-1-5-21-1111-2222-3333-1001";
    const OTHER: &str = "S-1-5-21-1111-2222-3333-1002";

    fn sec(sddl: &str) -> Security {
        parse_sddl(sddl, ME).unwrap()
    }

    #[test]
    fn a_file_in_the_profile_with_the_default_inherited_list_is_trusted() {
        let s = sec(&format!("O:{ME}G:{ME}D:(A;;FA;;;SY)(A;;FA;;;BA)(A;;FA;;;{ME})"));
        assert_eq!(s.owner, ME);
        assert_eq!(s.allows.len(), 3);
        assert!(plan_file_acl_ok(&s, ME, false).is_ok());
    }

    #[test]
    fn another_account_with_write_or_a_wide_group_is_refused_but_read_is_fine() {
        let write = sec(&format!("O:{ME}D:(A;;FA;;;{ME})(A;;FW;;;{OTHER})"));
        assert!(plan_file_acl_ok(&write, ME, false).unwrap_err().contains(OTHER));
        let everyone = sec(&format!("O:{ME}D:(A;;FA;;;{ME})(A;;GA;;;WD)"));
        assert!(plan_file_acl_ok(&everyone, ME, false).unwrap_err().contains("S-1-1-0"));
        let users_write = sec(&format!("O:{ME}D:(A;;0x2;;;BU)"));
        assert!(plan_file_acl_ok(&users_write, ME, false).is_err());
        let read_only = sec(&format!("O:{ME}D:(A;;FA;;;{ME})(A;;FR;;;BU)(A;;FRFX;;;{OTHER})"));
        assert!(plan_file_acl_ok(&read_only, ME, false).is_ok());
    }

    #[test]
    fn a_foreign_owner_is_refused_and_the_administrators_group_only_when_elevated() {
        let foreign = sec(&format!("O:{OTHER}D:(A;;FA;;;{ME})"));
        assert!(plan_file_acl_ok(&foreign, ME, true).unwrap_err().contains("owned"));
        let admins = sec(&format!("O:BAD:(A;;FA;;;{ME})(A;;FA;;;BA)"));
        assert!(plan_file_acl_ok(&admins, ME, false).is_err());
        assert!(plan_file_acl_ok(&admins, ME, true).is_ok());
    }

    #[test]
    fn inherit_only_deny_audit_and_label_entries_grant_nothing() {
        let s = sec(&format!("O:{ME}D:PAI(A;OICIIO;GA;;;WD)(D;;FW;;;{OTHER})(A;;FA;;;{ME})S:AI(AU;SAFA;FA;;;WD)(ML;;NW;;;LW)"));
        assert_eq!(s.allows, vec![Ace { trustee: ME.into(), mask: 0x001f_01ff }]);
        assert!(plan_file_acl_ok(&s, ME, false).is_ok());
    }

    #[test]
    fn a_missing_list_an_unknown_right_and_a_broken_entry_are_refused() {
        let none = sec(&format!("O:{ME}G:{ME}"));
        assert!(none.no_dacl && plan_file_acl_ok(&none, ME, false).is_err());
        assert!(parse_sddl(&format!("O:{ME}D:(A;;ZZ;;;WD)"), ME).is_err());
        assert!(parse_sddl(&format!("O:{ME}D:(A;;FA;;;WD"), ME).is_err());
        assert!(parse_sddl(&format!("O:{ME}D:(A;;FA)"), ME).is_err());
        assert!(parse_sddl("D:(A;;FA;;;WD)", ME).is_err(), "no owner");
        assert!(plan_file_acl_ok(&sec(&format!("O:{ME}D:(A;;FA;;;{ME})")), "", false).is_err(), "unknown caller");
    }

    #[test]
    fn sids_compare_without_regard_to_case_and_an_empty_trustee_is_the_caller() {
        let s = sec("O:s-1-5-21-1111-2222-3333-1001D:(A;;FA;;;)");
        assert!(plan_file_acl_ok(&s, ME, false).is_ok());
    }
}
