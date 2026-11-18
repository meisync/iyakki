//! Secrets: private and preshared keys, passwords. They are kept apart from the profile (the profile type has no field for
//! them), printed as `Secret(..)`, compared in constant time and overwritten when dropped.
//!
//! Dropping overwrites with volatile writes so the compiler cannot remove them; this is best effort (a copy the allocator
//! moved earlier is not tracked), which is why the API hands out references and never clones.

use crate::b64;

pub struct Secret(Vec<u8>);

impl Secret {
    pub fn new(bytes: Vec<u8>) -> Secret {
        Secret(bytes)
    }
    /// A WireGuard key from its base64 form: exactly 32 bytes, else `None`. The input string is not kept.
    pub fn from_key_b64(s: &str) -> Option<Secret> {
        let mut raw = b64::decode(s.trim())?;
        if raw.len() != 32 {
            wipe(&mut raw);
            return None;
        }
        Some(Secret(raw))
    }
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
    pub fn as_key(&self) -> Option<&[u8; 32]> {
        self.0.as_slice().try_into().ok()
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl PartialEq for Secret {
    fn eq(&self, o: &Secret) -> bool {
        ct_eq(&self.0, &o.0)
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

/// Overwrite a buffer in a way the optimiser keeps.
pub fn wipe(buf: &mut [u8]) {
    for b in buf.iter_mut() {
        // SAFETY: `b` is a valid, aligned, exclusive reference to one byte.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

/// Equality that does not stop at the first difference.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A byte buffer that is wiped when dropped: the netlink message that carries a private key.
pub struct SecretBuf(pub Vec<u8>);

impl SecretBuf {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Debug for SecretBuf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecretBuf({} bytes)", self.0.len())
    }
}

impl Drop for SecretBuf {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

/// Secrets that belong to one imported profile, handed to the caller (the store puts them in the vault).
#[derive(Debug, Default)]
pub struct ProfileSecrets {
    /// WireGuard: the interface private key.
    pub private_key: Option<Secret>,
    /// WireGuard: preshared keys by peer index.
    pub preshared: Vec<(usize, Secret)>,
    /// OpenVPN: inline blocks cut out of the config (`key`, `tls-auth`, `tls-crypt`, `tls-crypt-v2`, `pkcs12`), name and PEM text.
    pub blocks: Vec<(String, Secret)>,
}

impl ProfileSecrets {
    pub fn is_empty(&self) -> bool {
        self.private_key.is_none() && self.preshared.is_empty() && self.blocks.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_shows_the_bytes_and_keys_need_32_bytes() {
        let k = Secret::from_key_b64(&b64::encode(&[7u8; 32])).unwrap();
        assert_eq!(format!("{k:?}"), "Secret(..)");
        assert!(!format!("{:?}", ProfileSecrets { private_key: Some(k), ..Default::default() }).contains("BwcH"));
        assert!(Secret::from_key_b64(&b64::encode(&[7u8; 31])).is_none());
        assert!(Secret::from_key_b64("not base64!").is_none());
        assert_eq!(format!("{:?}", SecretBuf(vec![1, 2, 3])), "SecretBuf(3 bytes)");
    }

    #[test]
    fn equality_and_wipe() {
        assert!(Secret::new(vec![1, 2]) == Secret::new(vec![1, 2]));
        assert!(Secret::new(vec![1, 2]) != Secret::new(vec![1, 3]));
        assert!(Secret::new(vec![1, 2]) != Secret::new(vec![1]));
        let mut v = vec![9u8; 16];
        wipe(&mut v);
        assert!(v.iter().all(|&b| b == 0));
    }
}
