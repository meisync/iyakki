//! LUKS2 export of an encrypted home (F-STORE-05): the person's data as a standard LUKS2 file that `cryptsetup luksOpen` opens, so the
//! data is never locked to IYAKKI.
//!
//! The volume key exists only inside this crate: [`EncHome::export_luks2`] opens the container with the person's password, hands the key
//! straight to `iyakki_volcrypt::luks2_export` and wipes it; no function returns, logs or stores it. The new LUKS2 passphrase is the same
//! password the person typed (the recovery key and the administrator slot are not carried over; they stay IYAKKI's). The encrypted payload
//! is copied byte for byte (same cipher `aes-xts-plain64`, same data unit, same tweak), so nothing is decrypted or re-encrypted on the way.
//!
//! The steps, each checked: refuse an existing destination, write the 16 MiB LUKS2 header, copy the payload in 1 MiB pieces, flush, then
//! read the file back: the header must give the volume key back for the password (compared inside, never shown) and every payload byte
//! must equal the container's. A failure removes the partly written file.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::Path;

use iyakki_volcrypt::luks2::parse_header;
use iyakki_volcrypt::luks2_export::{export_header_sized, Costs, DATA_OFFSET, HEADER_COPY};
use iyakki_volcrypt::VolError;
use zeroize::Zeroizing;

use crate::{io_err, EncHome, HomeError, Secret};

/// What an export produced (no secret in it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportReport {
    /// Size of the file: the 16 MiB header area plus the payload.
    pub bytes: u64,
    pub payload_bytes: u64,
    /// The LUKS2 UUID (as `cryptsetup luksUUID` prints it).
    pub uuid: String,
}

const CHUNK: usize = 1 << 20;
/// The LUKS2 label of an exported home.
pub const LABEL: &str = "iyakki-home";

fn uuid_text(u: &[u8; 16]) -> String {
    let h: String = u.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

impl EncHome {
    /// Where an export of `user` is written by default: `<data>/home-enc/export/<account>.luks2` (never overwritten).
    pub fn export_path(&self, user: &str) -> std::path::PathBuf {
        self.dir().join("export").join(format!("{user}.luks2"))
    }

    /// Write the LUKS2 export of `user`'s home to `dest` (which must not exist). `password` is the person's home password.
    /// `progress` gets (bytes done, bytes total) after each piece.
    pub fn export_luks2(&self, user: &str, password: &str, dest: &Path, progress: &mut dyn FnMut(u64, u64)) -> Result<ExportReport, HomeError> {
        if password.is_empty() {
            return Err(HomeError::WrongSecret);
        }
        if self.exists(user) && self.is_open(user) {
            // an open home is being written to: a copy of its blocks now could be a half-finished state
            return Err(HomeError::InUse);
        }
        let vol = self.volume(user, &Secret::password(password))?;
        let h = vol.header().clone();
        let uuid = uuid_text(&h.uuid);
        let kdf = self.kdf();
        let costs = Costs { time: kdf.t, memory_kib: kdf.m_kib, cpus: kdf.p, digest_iterations: if self.kdf.is_some() { 1000 } else { 200_000 } };
        let header = {
            let failed = std::cell::Cell::new(false);
            let mut rng = |b: &mut [u8]| {
                if self.rng(|r| r.fill(b)).is_err() {
                    failed.set(true);
                }
            };
            // the key is read here and nowhere else
            let made = export_header_sized(vol.key(), password.as_bytes(), &uuid, LABEL, h.sector_size, costs, &mut rng);
            // a failing random source must never give a predictable header: nothing has been written yet
            if failed.get() {
                return Err(HomeError::Volume(VolError::Entropy.to_string()));
            }
            made.map_err(|e| HomeError::Volume(e.to_string()))?
        };
        let header = Zeroizing::new(header);
        let src = std::fs::File::open(self.container_path(user)).map_err(io_err)?;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(io_err)?;
        }
        let mut out = OpenOptions::new().write(true).create_new(true).mode(0o600).open(dest).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                HomeError::Io(format!("{} exists already; an export never overwrites a file", dest.display()))
            } else {
                io_err(e)
            }
        })?;
        let r = (|| -> Result<(), HomeError> {
            out.write_all(&header).map_err(io_err)?;
            let total = h.payload_bytes;
            let mut buf = vec![0u8; CHUNK];
            let mut done = 0u64;
            while done < total {
                let n = ((total - done) as usize).min(CHUNK);
                src.read_exact_at(&mut buf[..n], h.payload_offset + done).map_err(io_err)?;
                out.write_all(&buf[..n]).map_err(io_err)?;
                done += n as u64;
                progress(done, total);
            }
            out.sync_all().map_err(io_err)?;
            Ok(())
        })();
        let r = r.and_then(|_| verify(&src, dest, vol.key(), password.as_bytes(), h.payload_offset, h.payload_bytes));
        drop(out);
        if let Err(e) = r {
            let _ = std::fs::remove_file(dest);
            return Err(e);
        }
        Ok(ExportReport { bytes: DATA_OFFSET + h.payload_bytes, payload_bytes: h.payload_bytes, uuid })
    }
}

/// Read the export back: the header unlocks to the same key, the payload equals the container's.
fn verify(src: &std::fs::File, dest: &Path, key: &[u8; 64], password: &[u8], payload_offset: u64, payload_bytes: u64) -> Result<(), HomeError> {
    let f = std::fs::File::open(dest).map_err(io_err)?;
    let mut head = vec![0u8; HEADER_COPY];
    f.read_exact_at(&mut head, 0).map_err(io_err)?;
    let parsed = parse_header(&head).map_err(|e| HomeError::Volume(e.to_string()))?;
    let mut read = |o: u64, n: usize| -> Option<Vec<u8>> {
        let mut b = vec![0u8; n];
        f.read_exact_at(&mut b, o).ok()?;
        Some(b)
    };
    let back = parsed.unlock(password, &mut read).map_err(|e| HomeError::Volume(format!("the written header does not open: {e}")))?;
    if back.as_slice() != key.as_slice() {
        return Err(HomeError::Volume("the written header gives another key".into()));
    }
    let (mut a, mut b) = (vec![0u8; CHUNK], vec![0u8; CHUNK]);
    let mut done = 0u64;
    while done < payload_bytes {
        let n = ((payload_bytes - done) as usize).min(CHUNK);
        src.read_exact_at(&mut a[..n], payload_offset + done).map_err(io_err)?;
        f.read_exact_at(&mut b[..n], DATA_OFFSET + done).map_err(io_err)?;
        if a[..n] != b[..n] {
            return Err(HomeError::Volume("the copied payload differs from the container".into()));
        }
        done += n as u64;
    }
    Ok(())
}
