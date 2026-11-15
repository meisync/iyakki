//! The signed result file of CHECK-UP (F-CHECKUP-02): what a check-up found, in a form that maps to the cells of
//! `docs/project/HARDWARE-MATRIX.md` and that nobody can edit without it being noticed.
//!
//! The file is JSON: `{"body": "<JSON text>", "public": "<hex>", "signature": "<hex>"}`. The signature is Ed25519 over the exact bytes of
//! `body` (no canonical form to get wrong, like `dlmanifest`). The key is made on this computer the first time (`checkup-signing-key`, mode
//! 600, in the data folder) and never leaves it. That proves the file was not changed after the computer wrote it; it does **not** prove
//! which computer it was (anybody can make a key), so the owner compares `machine` with the machine they tested. A person pasting the file
//! into the matrix still decides the word of the cell: a pass is `verified` evidence, a fail is `failing`, anything else `untested`.
//!
//! Besides the twelve tile steps the body carries the cells that software alone can answer on this computer: `k3s start` (the dev cluster
//! state) and `GPU for k3s` (what the GPU line says). Whatever was not asked is `untested`, never guessed.

use std::path::Path;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::checkup::{Outcome, StepId, StepResult};

pub const SCHEMA: u32 = 1;
const KEY_FILE: &str = "checkup-signing-key";
pub const FILE_NAME: &str = "checkup-result.json";

/// The matrix cell(s) a tile step is evidence for.
pub fn cells(id: StepId) -> &'static [&'static str] {
    match id {
        StepId::Display => &["Display, internal"],
        StepId::Touch => &["Touch screen mapping"],
        StepId::Keyboard | StepId::Pointer => &["Keyboard (and pointer)"],
        StepId::Speaker => &["Sound out"],
        StepId::Microphone => &["Sound in (built-in)"],
        StepId::Camera => &["Built-in camera"],
        StepId::Monitor => &["Display, external monitor"],
        StepId::Network => &["Ethernet"],
        StepId::Wifi => &["Wi-Fi"],
        StepId::Bluetooth => &["Bluetooth"],
        StepId::Power => &["Battery and power source"],
        // the graphics card has its own cell, `GPU for k3s`, made from `Extra::gpu`; USB has no row in the hardware matrix yet
        StepId::Gpu | StepId::Usb => &[],
    }
}

fn word(o: Outcome) -> &'static str {
    match o {
        Outcome::Pass => "verified",
        Outcome::Fail => "failing",
        Outcome::Pending | Outcome::Skipped => "untested",
    }
}

/// Answers software alone can give.
#[derive(Clone, Debug, Default)]
pub struct Extra {
    /// The dev cluster state word (`running`, `stopped`, `failed`, ...); `None`: not asked.
    pub k3s_state: Option<String>,
    /// The GPU line in words and whether the GPU is usable by the cluster; `None`: not asked.
    pub gpu: Option<(String, bool)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cell {
    pub capability: String,
    /// `verified`, `failing` or `untested`.
    pub status: String,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Body {
    pub schema: u32,
    /// The machine as it names itself (the hardware summary's first line), for the owner to compare.
    pub machine: String,
    pub release: String,
    pub created: i64,
    pub cells: Vec<Cell>,
}

/// The cells of a body for these results.
pub fn build(results: &[StepResult], extra: &Extra, machine: &str, release: &str, now: i64) -> Body {
    let mut cells: Vec<Cell> = Vec::new();
    for r in results {
        for c in cells_of(r.id) {
            cells.push(Cell { capability: c.to_string(), status: word(r.outcome).into(), note: r.note.chars().take(200).collect() });
        }
    }
    let (k3s, note) = match extra.k3s_state.as_deref() {
        Some("running") => ("verified", "the dev cluster was running".to_string()),
        Some(s) => ("untested", format!("the dev cluster was {s}")),
        None => ("untested", "not asked".to_string()),
    };
    cells.push(Cell { capability: "k3s start".into(), status: k3s.into(), note });
    let (gpu, note) = match &extra.gpu {
        Some((line, true)) => ("verified", line.chars().take(200).collect::<String>()),
        Some((line, false)) => ("untested", line.chars().take(200).collect::<String>()),
        None => ("untested", "not asked".to_string()),
    };
    cells.push(Cell { capability: "GPU for k3s".into(), status: gpu.into(), note });
    Body { schema: SCHEMA, machine: machine.chars().filter(|c| !c.is_control()).take(120).collect(), release: release.into(), created: now, cells }
}

fn cells_of(id: StepId) -> impl Iterator<Item = &'static str> {
    cells(id).iter().copied()
}

fn key_in(dir: &Path) -> Result<SigningKey, String> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let p = dir.join(KEY_FILE);
    if let Ok(t) = std::fs::read_to_string(&p) {
        if let Some(b) = crate::pkg::unhex::<32>(t.trim()) {
            return Ok(SigningKey::from_bytes(&b));
        }
        return Err(format!("{} is damaged: delete it to make a new key", p.display()));
    }
    let mut seed = [0u8; 32];
    iyakki_sec::random_fill(&mut seed).map_err(|e| format!("no random numbers: {e:?}"))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    std::io::Write::write_all(&mut f, crate::pkg::hex(&seed).as_bytes()).map_err(|e| format!("{}: {e}", p.display()))?;
    let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
    Ok(SigningKey::from_bytes(&seed))
}

/// The signed file text for a body, with the key kept in `dir`.
pub fn sign(dir: &Path, body: &Body) -> Result<String, String> {
    let key = key_in(dir)?;
    let text = serde_json::to_string(body).map_err(|e| e.to_string())?;
    let sig = key.sign(text.as_bytes());
    serde_json::to_string_pretty(&serde_json::json!({"body": text, "public": crate::pkg::hex(key.verifying_key().as_bytes()), "signature": crate::pkg::hex(&sig.to_bytes())})).map_err(|e| e.to_string())
}

/// Check a file and give its body. Fails when the signature does not match the text or the file is not what it should be.
pub fn verify(file_text: &str) -> Result<Body, String> {
    let v: serde_json::Value = serde_json::from_str(file_text).map_err(|e| format!("not a check-up result file: {e}"))?;
    let field = |n: &str| v.get(n).and_then(|x| x.as_str()).ok_or_else(|| format!("the file has no {n}"));
    let (body, public, sig) = (field("body")?, field("public")?, field("signature")?);
    let pk = crate::pkg::unhex::<32>(public).and_then(|b| VerifyingKey::from_bytes(&b).ok()).ok_or("the public key is not valid")?;
    let sg = crate::pkg::unhex::<64>(sig).map(|b| Signature::from_bytes(&b)).ok_or("the signature is not 128 hexadecimal characters")?;
    pk.verify_strict(body.as_bytes(), &sg).map_err(|_| "the signature does not match: the file was changed after it was written".to_string())?;
    let b: Body = serde_json::from_str(body).map_err(|e| format!("the signed text is not a result: {e}"))?;
    if b.schema != SCHEMA {
        return Err(format!("result schema {} is not known to this release", b.schema));
    }
    Ok(b)
}

/// One line per cell, for pasting next to the matrix row of the machine.
pub fn lines(b: &Body) -> Vec<String> {
    let mut out = vec![format!("{} / {} / {}", b.machine, b.release, crate::release::civil_date(b.created.max(0) as u64))];
    out.extend(b.cells.iter().map(|c| format!("| {} | {} | {} |", c.capability, c.status, c.note)));
    out
}

/// Write the signed result into `dir` (replacing the last one) when every step has an answer. `Ok(None)` while steps are still pending.
pub fn save_if_complete(dir: &Path, results: &[StepResult], extra: &Extra, machine: &str, release: &str, now: i64) -> Result<Option<std::path::PathBuf>, String> {
    if results.iter().any(|r| r.outcome == Outcome::Pending) {
        return Ok(None);
    }
    let text = sign(dir, &build(results, extra, machine, release, now))?;
    let p = dir.join(FILE_NAME);
    let tmp = dir.join(format!("{FILE_NAME}.part"));
    std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, &p)).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(Some(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-ckres-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn results(fail: Option<StepId>, pending: bool) -> Vec<StepResult> {
        StepId::ALL
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let mut r = StepResult::pending(*id);
                r.outcome = if Some(*id) == fail { Outcome::Fail } else if pending && i == 0 { Outcome::Pending } else if i == 5 { Outcome::Skipped } else { Outcome::Pass };
                r.note = format!("note {i}");
                r
            })
            .collect()
    }

    #[test]
    fn every_step_maps_to_a_matrix_cell_that_exists_in_the_matrix_page() {
        let page = include_str!("../../../docs/project/HARDWARE-MATRIX.md");
        for id in StepId::ALL {
            for c in cells(id) {
                assert!(page.contains(&format!("| {c} |")), "{c} is not a row of the matrix");
            }
        }
        for c in ["k3s start", "GPU for k3s"] {
            assert!(page.contains(&format!("| {c} |")), "{c}");
        }
    }

    #[test]
    fn a_complete_check_up_is_signed_and_verifies_and_the_words_follow_the_outcomes() {
        let d = dir("ok");
        let p = save_if_complete(&d, &results(Some(StepId::Wifi), false), &Extra { k3s_state: Some("running".into()), gpu: Some(("AMD, Vulkan".into(), true)) }, "Test machine", "1.2.3", 1_800_000_000).unwrap().unwrap();
        let b = verify(&std::fs::read_to_string(&p).unwrap()).unwrap();
        let st = |c: &str| b.cells.iter().find(|x| x.capability == c).unwrap().status.clone();
        assert_eq!((st("Wi-Fi").as_str(), st("Display, internal").as_str(), st("Sound in (built-in)").as_str()), ("failing", "verified", "untested"));
        assert_eq!((st("k3s start").as_str(), st("GPU for k3s").as_str()), ("verified", "verified"));
        assert_eq!(b.machine, "Test machine");
        assert!(lines(&b)[1].starts_with("| "));
        // the same key signs the next one
        let again = std::fs::read_to_string(&p).unwrap();
        save_if_complete(&d, &results(None, false), &Extra::default(), "Test machine", "1.2.3", 1).unwrap();
        let pub1: serde_json::Value = serde_json::from_str(&again).unwrap();
        let pub2: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(pub1["public"], pub2["public"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_changed_file_a_foreign_key_or_a_half_check_up_is_not_accepted() {
        let d = dir("bad");
        assert_eq!(save_if_complete(&d, &results(None, true), &Extra::default(), "m", "r", 1).unwrap(), None, "steps are still pending: nothing written");
        assert!(!d.join(FILE_NAME).exists());
        let text = sign(&d, &build(&results(None, false), &Extra::default(), "m", "r", 1)).unwrap();
        assert!(verify(&text).is_ok());
        let edited = text.replace("\\\"status\\\":\\\"untested\\\"", "\\\"status\\\":\\\"verified\\\"");
        assert_ne!(edited, text);
        assert!(verify(&edited).unwrap_err().contains("changed after"), "{}", verify(&edited).unwrap_err());
        assert!(verify("hello").is_err() && verify("{}").is_err());
        let other = dir("bad2");
        let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let k2 = key_in(&other).unwrap();
        v["public"] = crate::pkg::hex(k2.verifying_key().as_bytes()).into();
        assert!(verify(&v.to_string()).is_err(), "the signature belongs to another key");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(d.join(KEY_FILE)).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&other);
    }

    fn compat_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/compat/alpha3/checkup-result.json")
    }

    /// Writes `tests/compat/alpha3/checkup-result.json`, a signed result made by this release's code with a throw-away key.
    #[test]
    #[ignore]
    fn record_alpha3_checkup_result() {
        if std::env::var("IYAKKI_COMPAT_RECORD").is_ok() {
            let d = dir("record");
            let text = sign(&d, &build(&results(Some(StepId::Wifi), false), &Extra { k3s_state: Some("running".into()), gpu: Some(("example GPU".into(), true)) }, "Example machine", "0.1.0-alpha.3", 1_800_000_000)).unwrap();
            std::fs::write(compat_path(), text).unwrap();
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    #[test]
    fn the_signed_result_of_alpha3_still_verifies_and_a_result_of_a_newer_schema_is_refused_by_name() {
        let text = std::fs::read_to_string(compat_path()).unwrap();
        let b = verify(&text).unwrap();
        assert_eq!((b.schema, b.machine.as_str(), b.release.as_str()), (1, "Example machine", "0.1.0-alpha.3"));
        assert!(b.cells.iter().any(|c| c.capability == "Wi-Fi" && c.status == "failing") && b.cells.iter().any(|c| c.capability == "k3s start" && c.status == "verified"));
        // unknown fields a newer release adds to the signed text are ignored once the signature holds
        let d = dir("newer");
        let mut body = build(&results(None, false), &Extra::default(), "m", "r", 1);
        body.schema = SCHEMA + 1;
        let newer = sign(&d, &body).unwrap();
        assert!(verify(&newer).unwrap_err().contains("is not known to this release"), "{}", verify(&newer).unwrap_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
