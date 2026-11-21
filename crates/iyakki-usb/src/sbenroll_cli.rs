//! Command line of Secure Boot route 3 (F-BOOT-12), used by `dist/buildroot/secureboot.sh enrol-files` and by a person at the machine:
//!
//! ```text
//! iyakki-usb --sb-status                       what the firmware allows (free, read only)
//! iyakki-usb --sb-prepare CERT.der OUTDIR      writes OUTDIR/{PK,KEK,db}.{list,time,tosign} (the owner's own step signs the .tosign files)
//! iyakki-usb --sb-assemble OUTDIR              joins each .list, .time and the owner's .p7 (DER PKCS#7) into OUTDIR/NAME.auth
//! iyakki-usb --sb-enrol OUTDIR --yes           writes the three .auth files into the firmware (setup mode only; asks for --yes)
//! ```
//!
//! The signing key is never an argument and never read here: the `.p7` files are made by the key owner's own step (openssl in the
//! script, with the key path from the environment).

use crate::sbenroll::{self, Var};
use crate::sbenroll_write::{self, EfiVarFs, Signed};
use std::path::Path;

const VARS: [Var; 3] = [Var::Pk, Var::Kek, Var::Db];

/// A Unix time as a UTC calendar time (proleptic Gregorian; the days-to-civil algorithm).
pub fn utc_parts(unix: u64) -> (u16, u8, u8, u8, u8, u8) {
    let (days, rem) = ((unix / 86_400) as i64, (unix % 86_400) as u32);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let y = if m <= 2 { y + 1 } else { y } as u16;
    (y, m, d, (rem / 3600) as u8, (rem % 3600 / 60) as u8, (rem % 60) as u8)
}

/// Write the lists, times and to-sign files for one enrolment.
pub fn prepare(cert_der: &[u8], out: &Path, unix_now: u64) -> Result<(), String> {
    if cert_der.len() < 4 || cert_der[0] != 0x30 {
        return Err("the certificate must be DER (openssl x509 -outform DER)".into());
    }
    let owner = owner_guid(cert_der);
    let (y, mo, d, h, mi, s) = utc_parts(unix_now);
    let time = sbenroll::efi_time(y, mo, d, h, mi, s);
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    for (var, list) in sbenroll::enrolment_lists(cert_der, &owner)? {
        let w = |ext: &str, b: &[u8]| std::fs::write(out.join(format!("{}.{ext}", var.name())), b).map_err(|e| e.to_string());
        w("list", &list)?;
        w("time", &time)?;
        w("tosign", &sbenroll::data_to_sign(var, &time, &list)?)?;
    }
    Ok(())
}

/// A stable owner GUID taken from the certificate bytes (the owner field is informational; the same certificate always gets the same one).
fn owner_guid(cert: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(cert);
    let hex: String = h[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

/// Join list, time and the owner's signature into `NAME.auth` files.
pub fn assemble(dir: &Path) -> Result<(), String> {
    for var in VARS {
        let rd = |ext: &str| std::fs::read(dir.join(format!("{}.{ext}", var.name()))).map_err(|e| format!("{}.{ext}: {e}", var.name()));
        let (list, time, p7) = (rd("list")?, rd("time")?, rd("p7")?);
        let time: [u8; 16] = time.try_into().map_err(|_| format!("{}.time is not 16 bytes", var.name()))?;
        let auth = sbenroll::auth_file(&time, &p7, &list)?;
        std::fs::write(dir.join(format!("{}.auth", var.name())), auth).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn load_signed(dir: &Path) -> Result<Signed, String> {
    let rd = |v: Var| std::fs::read(dir.join(format!("{}.auth", v.name()))).map_err(|e| format!("{}.auth: {e} (run --sb-assemble first)", v.name()));
    Ok(Signed { pk: rd(Var::Pk)?, kek: rd(Var::Kek)?, db: rd(Var::Db)? })
}

pub fn run(args: &[String]) -> i32 {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    let efi = crate::gpt_install::booted_uefi();
    let r: Result<String, String> = match a.as_slice() {
        ["--sb-status"] => Ok(sbenroll_write::status_words(&EfiVarFs::real(), efi)),
        ["--sb-prepare", cert, out] => std::fs::read(cert).map_err(|e| format!("{cert}: {e}")).and_then(|c| {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            prepare(&c, Path::new(out), now).map(|_| format!("wrote the lists and the files to sign in {out}"))
        }),
        ["--sb-assemble", dir] => assemble(Path::new(dir)).map(|_| format!("wrote the .auth files in {dir}")),
        ["--sb-enrol", dir, "--yes"] => load_signed(Path::new(dir)).and_then(|s| sbenroll_write::enrol(&mut EfiVarFs::real(), efi, &s)).map(|r| format!("written: {}. The firmware is now in user mode with IYAKKI's key.", r.written.join(", "))),
        ["--sb-enrol", ..] => Err("this writes Secure Boot keys into the firmware and cannot be undone from the running system: add --yes after you have read docs/SECURE-BOOT.md".into()),
        _ => Err("usage: iyakki-usb --sb-status | --sb-prepare CERT.der OUTDIR | --sb-assemble OUTDIR | --sb-enrol OUTDIR --yes".into()),
    };
    match r {
        Ok(t) => {
            println!("{t}");
            0
        }
        Err(e) => {
            eprintln!("iyakki-usb: {e}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_time_becomes_a_utc_calendar_time_including_leap_days() {
        assert_eq!(utc_parts(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(utc_parts(951_782_400), (2000, 2, 29, 0, 0, 0));
        let (y, m, d, h, mi, s) = utc_parts(1_798_761_599);
        assert_eq!((y, m, d, h, mi, s), (2026, 12, 31, 23, 59, 59));
    }

    #[test]
    fn prepare_then_a_signature_then_assemble_gives_three_auth_files_the_writer_accepts() {
        let d = std::env::temp_dir().join(format!("iyakki-usb-sbcli-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        prepare(&[0x30, 0x03, 1, 2, 3], &d, 1_798_761_599).unwrap();
        for v in ["PK", "KEK", "db"] {
            for e in ["list", "time", "tosign"] {
                assert!(d.join(format!("{v}.{e}")).is_file(), "{v}.{e}");
            }
            assert_eq!(std::fs::read(d.join(format!("{v}.time"))).unwrap().len(), 16);
            // what is signed starts with the variable's UTF-16 name
            assert_eq!(std::fs::read(d.join(format!("{v}.tosign"))).unwrap()[0], v.as_bytes()[0]);
            std::fs::write(d.join(format!("{v}.p7")), [0x30, 0x03, 9, 9, 9]).unwrap();
        }
        assemble(&d).unwrap();
        let s = load_signed(&d).unwrap();
        assert!(s.pk.len() > 36 && s.kek.len() > 36 && s.db.len() > 36);
        assert_eq!(&s.pk[..2], &2026u16.to_le_bytes());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_pem_certificate_a_missing_signature_and_a_missing_yes_are_refused_in_plain_words() {
        let d = std::env::temp_dir().join(format!("iyakki-usb-sbcli2-{}", std::process::id()));
        assert!(prepare(b"-----BEGIN CERTIFICATE-----", &d, 0).unwrap_err().contains("DER"));
        prepare(&[0x30, 0x03, 1, 2, 3], &d, 0).unwrap();
        assert!(assemble(&d).unwrap_err().contains(".p7"));
        assert_eq!(run(&["--sb-enrol".into(), d.display().to_string()]), 2);
        let _ = std::fs::remove_dir_all(&d);
    }
}
