//! A fake machine for the tests of other crates (the Settings page): a lifecycle over a recording kernel, a small Argon2id cost, a fixed
//! random sequence and plenty of free space. Nothing here is used by the real system.

use std::path::Path;
use std::sync::Arc;

use iyakki_storage::dm::DmKernel;
use iyakki_volcrypt::header::KdfParams;
use iyakki_volcrypt::{Random, VolError};

use crate::EncHome;

struct Seq(u64);

impl Random for Seq {
    fn fill(&mut self, buf: &mut [u8]) -> Result<(), VolError> {
        for b in buf.iter_mut() {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            *b = (self.0 >> 24) as u8;
        }
        Ok(())
    }
}

/// A lifecycle on `kernel` (a fake, possibly a slow one) with the data folder `data_dir` and the homes under `home_parent`.
pub fn fake_env(data_dir: &Path, home_parent: &Path, kernel: Arc<dyn DmKernel>) -> EncHome {
    EncHome::with(data_dir, home_parent, kernel, KdfParams { m_kib: 64, t: 1, p: 1 }, Box::new(Seq(0x9e37_79b9_7f4a_7c15)), Box::new(|_| Some(10 << 30)), Box::new(|| 1000))
}
