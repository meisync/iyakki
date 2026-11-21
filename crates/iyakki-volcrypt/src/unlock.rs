//! Where the secret for a whole-partition volume comes from at boot (F-STORE-05, design section 4: "stage1 prompt" and section 12 item 6).
//!
//! stage1 has no on-screen keyboard and no desktop, so the way the secret arrives is a choice, made once and stored in the clear next
//! to the volume (it is not a secret; choosing it wrongly only means the volume does not open). [`UnlockSource`] is what stage1 asks;
//! three sources implement it behind the same trait:
//!
//! * [`Keyboard`] (the DEFAULT, and what an unknown or missing setting means): a passphrase typed on the console, a limited number of
//!   tries, the typed bytes wiped after use. Needs a keyboard that stage1 can read.
//! * [`KeyFile`]: a key file on a removable stick whose label is known; unattended while the stick is plugged in. The file's bytes are
//!   the password of an ordinary password slot, so the volume needs no special format.
//! * [`Tpm`]: a secret released by the machine's TPM (through [`TpmBackend`]) only if the boot measurements match; a backend that says
//!   "not available" makes the caller fall back to the keyboard, never to "no encryption".
//!
//! The real console, stick and TPM are small adapters behind the three input traits; everything that decides is here and runs on fakes.

use zeroize::Zeroizing;

use crate::container::{Store, Volume};
use crate::VolError;

/// Why a source gave no secret.
#[derive(Debug, PartialEq, Eq)]
pub enum UnlockError {
    /// This source cannot work on this machine now (no keyboard, no stick, no TPM): the caller may try the next source.
    Unavailable(String),
    /// The person gave up (empty line, end of input).
    Cancelled,
}

/// One way to obtain the secret.
pub trait UnlockSource {
    fn name(&self) -> &'static str;
    /// The secret for try number `attempt` (counting from 0). Called again after a wrong secret.
    fn secret(&mut self, attempt: u32) -> Result<Zeroizing<Vec<u8>>, UnlockError>;
}

/// The setting that says which source to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Keyboard,
    KeyFile,
    Tpm,
}

impl Kind {
    /// From the stored setting. Anything that is not exactly a known word is the conservative default, the keyboard.
    pub fn parse(setting: &str) -> Kind {
        match setting.trim().to_ascii_lowercase().as_str() {
            "keyfile" | "key-file" | "usb-key" => Kind::KeyFile,
            "tpm" => Kind::Tpm,
            _ => Kind::Keyboard,
        }
    }
    pub fn word(self) -> &'static str {
        match self {
            Kind::Keyboard => "keyboard",
            Kind::KeyFile => "keyfile",
            Kind::Tpm => "tpm",
        }
    }
}

/// The console: the one line a person types.
pub trait LineInput {
    /// Show `prompt` and read a line without echo. `None` is end of input (no keyboard, closed console).
    fn read_secret(&mut self, prompt: &str) -> Option<Zeroizing<Vec<u8>>>;
}

/// A passphrase from the keyboard.
pub struct Keyboard<I: LineInput> {
    pub input: I,
}

impl<I: LineInput> UnlockSource for Keyboard<I> {
    fn name(&self) -> &'static str {
        "keyboard"
    }
    fn secret(&mut self, attempt: u32) -> Result<Zeroizing<Vec<u8>>, UnlockError> {
        let prompt = if attempt == 0 { "Passphrase for the encrypted disk: " } else { "That did not open the disk. Passphrase: " };
        match self.input.read_secret(prompt) {
            None => Err(UnlockError::Unavailable("there is no keyboard to type on".into())),
            Some(s) if s.is_empty() => Err(UnlockError::Cancelled),
            Some(s) => Ok(s),
        }
    }
}

/// The removable drives and the file on them.
pub trait StickFiles {
    /// The bytes of `path` on the removable volume labelled `label`, or `None` when the volume or the file is not there.
    fn read(&mut self, label: &str, path: &str) -> Option<Zeroizing<Vec<u8>>>;
}

/// A key file on a stick.
pub struct KeyFile<F: StickFiles> {
    pub files: F,
    pub label: String,
    pub path: String,
}

impl<F: StickFiles> UnlockSource for KeyFile<F> {
    fn name(&self) -> &'static str {
        "key file"
    }
    fn secret(&mut self, _attempt: u32) -> Result<Zeroizing<Vec<u8>>, UnlockError> {
        match self.files.read(&self.label, &self.path) {
            Some(b) if !b.is_empty() && b.len() <= 4096 => Ok(b),
            Some(_) => Err(UnlockError::Unavailable(format!("the key file {} on {} is empty or too big", self.path, self.label))),
            None => Err(UnlockError::Unavailable(format!("the key stick {} (file {}) is not plugged in", self.label, self.path))),
        }
    }
}

/// The TPM: release the secret sealed to the boot measurements.
pub trait TpmBackend {
    /// `Err` says in words why not (no TPM, the measurements changed, the chip is locked out).
    fn unseal(&mut self) -> Result<Zeroizing<Vec<u8>>, String>;
}

pub struct Tpm<T: TpmBackend> {
    pub tpm: T,
}

impl<T: TpmBackend> UnlockSource for Tpm<T> {
    fn name(&self) -> &'static str {
        "TPM"
    }
    fn secret(&mut self, _attempt: u32) -> Result<Zeroizing<Vec<u8>>, UnlockError> {
        self.tpm.unseal().map_err(UnlockError::Unavailable)
    }
}

/// How an unlock ended without a volume.
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    /// The header is not ours or is damaged: no source can fix that.
    Volume(VolError),
    /// Every source was unavailable or cancelled; each said why.
    NoSource(Vec<String>),
    /// The secret was wrong this many times.
    TooManyTries(u32),
}

/// What an unlock picked: the volume, and which source opened it.
pub struct Opened<S: Store> {
    pub volume: Volume<S>,
    pub source: &'static str,
}

/// Try the sources in order. A source that is unavailable hands over to the next one; the keyboard (when it is in the list) gets up to
/// `max_tries` tries, a stick or TPM secret that is wrong is not retried (it would be the same bytes) and hands over to the next source.
/// The store is cloned for every try (a `Clone` store such as a file handle wrapper) because opening consumes it.
pub fn unlock<S: Store + Clone>(store: &S, sources: &mut [&mut dyn UnlockSource], max_tries: u32) -> Result<Opened<S>, Failure> {
    let mut notes = Vec::new();
    for src in sources.iter_mut() {
        let name = src.name();
        let interactive = name == "keyboard";
        let tries = if interactive { max_tries.max(1) } else { 1 };
        let mut attempt = 0;
        while attempt < tries {
            match src.secret(attempt) {
                Ok(secret) => match Volume::open_password(store.clone(), &secret) {
                    Ok(volume) => return Ok(Opened { volume, source: name }),
                    Err(VolError::WrongSecret) => {
                        attempt += 1;
                        if attempt == tries {
                            if interactive {
                                return Err(Failure::TooManyTries(tries));
                            }
                            notes.push(format!("the {name} did not open the disk"));
                        }
                    }
                    Err(e) => return Err(Failure::Volume(e)),
                },
                Err(UnlockError::Unavailable(why)) => {
                    notes.push(format!("{name}: {why}"));
                    break;
                }
                Err(UnlockError::Cancelled) => {
                    notes.push(format!("{name}: cancelled"));
                    break;
                }
            }
        }
    }
    Err(Failure::NoSource(notes))
}

/// The sources for a setting, always ending with the keyboard so that a missing stick or TPM never locks the person out.
/// Returns the order as words (for the boot log); the caller builds the real adapters in this order.
pub fn order(kind: Kind) -> Vec<Kind> {
    match kind {
        Kind::Keyboard => vec![Kind::Keyboard],
        other => vec![other, Kind::Keyboard],
    }
}
