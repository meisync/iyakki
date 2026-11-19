//! Secure Boot, route 3 (F-BOOT-12): enrol our key in the firmware itself, with no shim, on machines that allow custom keys.
//!
//! What is here is the part that needs no firmware: the file formats (an EFI signature list holding one X.509 certificate, the
//! bytes a time-based authenticated write is signed over, the `EFI_VARIABLE_AUTHENTICATION_2` descriptor around a detached
//! PKCS#7 signature that the key owner's tool makes) and the verdict of what the firmware allows (setup mode, user mode, which
//! keys exist). It reads and writes nothing on a machine: the variables come in through [`VarSource`] and the files go out as
//! bytes. The key itself is never read here; the signature arrives from the owner's own signing step.
//!
//! The honest limit: whether a given firmware accepts the write is only known by trying it on that machine (Manual: needed).

/// The EFI global-variable GUID (PK, KEK, SetupMode, SecureBoot).
pub const GLOBAL_GUID: &str = "8be4df61-93ca-11d2-aa0d-00e098032b8c";
/// The image security database GUID (db, dbx).
pub const DB_GUID: &str = "d719b2cb-3d3a-4596-a3bc-dad00e67656f";
const CERT_X509_GUID: &str = "a5c059a1-94e4-4aa7-87b5-ab155c2bf072";
const CERT_PKCS7_GUID: &str = "4aafd29d-68df-49ee-8aa9-347d375665a7";
/// NON_VOLATILE | BOOTSERVICE_ACCESS | RUNTIME_ACCESS | TIME_BASED_AUTHENTICATED_WRITE_ACCESS.
pub const AUTH_ATTRS: u32 = 0x27;

/// A GUID text to its 16 on-disk bytes (the first three fields little-endian).
pub fn guid_bytes(g: &str) -> Result<[u8; 16], String> {
    let hex: String = g.chars().filter(|c| *c != '-').collect();
    if hex.len() != 32 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("{g} is not a GUID"));
    }
    let b: Vec<u8> = (0..16).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap_or(0)).collect();
    let mut o = [0u8; 16];
    o[0..4].copy_from_slice(&[b[3], b[2], b[1], b[0]]);
    o[4..6].copy_from_slice(&[b[5], b[4]]);
    o[6..8].copy_from_slice(&[b[7], b[6]]);
    o[8..16].copy_from_slice(&b[8..16]);
    Ok(o)
}

/// One EFI signature list holding one DER certificate, owned by `owner_guid`.
pub fn signature_list(cert_der: &[u8], owner_guid: &str) -> Result<Vec<u8>, String> {
    if cert_der.len() < 4 || cert_der[0] != 0x30 {
        return Err("the certificate is not DER (it must start with a SEQUENCE; a PEM file is converted first)".into());
    }
    let sig_size = 16 + cert_der.len();
    let list_size = 28 + sig_size;
    let mut v = Vec::with_capacity(list_size);
    v.extend_from_slice(&guid_bytes(CERT_X509_GUID)?);
    v.extend_from_slice(&(list_size as u32).to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&(sig_size as u32).to_le_bytes());
    v.extend_from_slice(&guid_bytes(owner_guid)?);
    v.extend_from_slice(cert_der);
    Ok(v)
}

/// A UTC time as the firmware's `EFI_TIME` (16 bytes).
pub fn efi_time(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> [u8; 16] {
    let mut t = [0u8; 16];
    t[0..2].copy_from_slice(&year.to_le_bytes());
    t[2] = month;
    t[3] = day;
    t[4] = hour;
    t[5] = minute;
    t[6] = second;
    t
}

/// The variable names this route writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Var {
    Pk,
    Kek,
    Db,
}

impl Var {
    pub fn name(self) -> &'static str {
        match self {
            Var::Pk => "PK",
            Var::Kek => "KEK",
            Var::Db => "db",
        }
    }
    pub fn guid(self) -> &'static str {
        if self == Var::Db { DB_GUID } else { GLOBAL_GUID }
    }
}

/// The bytes the owner's key signs for a time-based authenticated write: name (UTF-16, no terminator), vendor GUID,
/// attributes, the timestamp and the new contents. Feed these to `openssl smime -sign -binary -noattr -outform DER`.
pub fn data_to_sign(var: Var, time: &[u8; 16], payload: &[u8]) -> Result<Vec<u8>, String> {
    let mut v = Vec::new();
    for u in var.name().encode_utf16() {
        v.extend_from_slice(&u.to_le_bytes());
    }
    v.extend_from_slice(&guid_bytes(var.guid())?);
    v.extend_from_slice(&AUTH_ATTRS.to_le_bytes());
    v.extend_from_slice(time);
    v.extend_from_slice(payload);
    Ok(v)
}

/// The `.auth` file: `EFI_VARIABLE_AUTHENTICATION_2` (timestamp and a PKCS#7 `WIN_CERTIFICATE_UEFI_GUID`) followed by the contents.
/// This is what `efi-updatevar -f` and the firmware's own "append from file" take.
pub fn auth_file(time: &[u8; 16], pkcs7_der: &[u8], payload: &[u8]) -> Result<Vec<u8>, String> {
    if pkcs7_der.len() < 4 || pkcs7_der[0] != 0x30 {
        return Err("the signature is not a DER PKCS#7 structure".into());
    }
    let cert_len = 8 + 16 + pkcs7_der.len();
    let mut v = Vec::with_capacity(16 + cert_len + payload.len());
    v.extend_from_slice(time);
    v.extend_from_slice(&(cert_len as u32).to_le_bytes());
    v.extend_from_slice(&0x0200u16.to_le_bytes());
    v.extend_from_slice(&0x0EF1u16.to_le_bytes());
    v.extend_from_slice(&guid_bytes(CERT_PKCS7_GUID)?);
    v.extend_from_slice(pkcs7_der);
    v.extend_from_slice(payload);
    Ok(v)
}

/// Where the firmware's variables come from (the real one reads efivarfs; tests give a map).
pub trait VarSource {
    /// The contents of the variable without its four attribute bytes, or `None` when it does not exist.
    fn read(&self, name: &str, guid: &str) -> Option<Vec<u8>>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The machine did not start through UEFI, or exposes no variables.
    NoUefi,
    /// Setup mode: no platform key, so our key can be enrolled as PK, KEK and db from here (the firmware accepts the first PK unsigned by another).
    SetupMode,
    /// User mode with Secure Boot on: only the firmware's own menu can clear or replace keys.
    UserModeSecureBoot,
    /// User mode with Secure Boot off: the firmware menu is still the route (custom mode or "delete keys").
    UserMode,
}

impl Verdict {
    /// What to tell the person, in plain words.
    pub fn words(&self) -> &'static str {
        match self {
            Verdict::NoUefi => "This computer did not start through UEFI, so Secure Boot keys cannot be enrolled from here.",
            Verdict::SetupMode => "The firmware is in setup mode (it has no platform key), so IYAKKI's key can be enrolled directly. Keep the signing key safe: it becomes the owner of this computer's Secure Boot.",
            Verdict::UserModeSecureBoot => "Secure Boot is on and the firmware has a platform key. Enrolling a key directly means opening the firmware's own setup screen (custom keys or \"clear keys\"), then running this again. Or use the shim route (one-time key enrolment on the blue screen).",
            Verdict::UserMode => "The firmware has a platform key, but Secure Boot is off. Open the firmware's setup screen, choose custom keys or clear the keys to reach setup mode, then run this again.",
        }
    }
}

fn one_byte(v: Option<Vec<u8>>) -> Option<u8> {
    v.and_then(|b| b.first().copied())
}

/// Read the firmware's state and say which route is open.
pub fn judge(vars: &dyn VarSource, efi_present: bool) -> Verdict {
    if !efi_present {
        return Verdict::NoUefi;
    }
    let setup = one_byte(vars.read("SetupMode", GLOBAL_GUID));
    let secure = one_byte(vars.read("SecureBoot", GLOBAL_GUID));
    match (setup, secure) {
        (Some(1), _) => Verdict::SetupMode,
        (None, _) => Verdict::NoUefi,
        (Some(_), Some(1)) => Verdict::UserModeSecureBoot,
        _ => Verdict::UserMode,
    }
}

/// The three signature lists of one enrolment (the same certificate as platform key, key-exchange key and allowed signer).
pub fn enrolment_lists(cert_der: &[u8], owner_guid: &str) -> Result<[(Var, Vec<u8>); 3], String> {
    let l = signature_list(cert_der, owner_guid)?;
    Ok([(Var::Pk, l.clone()), (Var::Kek, l.clone()), (Var::Db, l)])
}
