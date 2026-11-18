//! The `iyakki-core` shared library's C interface (F-CORE-27). Plain data only (docs/development/C-INTERFACE-RULES.md): integers, pointers
//! with a length, UTF-8 JSON. It carries what a program in another language needs before it talks to the core at all: which interface
//! version it is loading, and the contract's `hello` (the version range, the domains and calls this build knows, the rate limit, the
//! deprecated names). Calls and events follow in later functions with their own names; nothing here changes a name or a meaning
//! (the gate is `crates/iyakki-core-api/tests/c_abi.rs`, the header comes from `dist/release/make-core-header.sh`).
//!
//! No state, no device, no file: a loaded library can be asked its version by anyone, and `hello` is the same description the developer
//! mode channel already gives a paired computer, without the machine name or build.

use iyakki_core_api::contract::catalog::hello_for;
use iyakki_core_api::identity::Capabilities;

/// The version of this C interface. Minor grows with new functions; major changes only with a new library name.
pub const ABI_MAJOR: u32 = 0;
pub const ABI_MINOR: u32 = 1;
pub const ABI_PATCH: u32 = 0;

/// Return codes: 0 is ok, negative is a refusal (rule 6).
pub const OK: i32 = 0;
pub const ERR_NULL: i32 = -1;

/// Write the interface version (major, minor, patch) to three `u32` places. Apps check it at start.
///
/// # Safety
/// `major`, `minor` and `patch` must each be null or point to a writable `u32`.
#[no_mangle]
pub unsafe extern "C" fn iyakki_core_abi_version(major: *mut u32, minor: *mut u32, patch: *mut u32) -> i32 {
    if major.is_null() || minor.is_null() || patch.is_null() {
        return ERR_NULL;
    }
    *major = ABI_MAJOR;
    *minor = ABI_MINOR;
    *patch = ABI_PATCH;
    OK
}

/// The contract's `hello` as UTF-8 JSON. Writes at most `cap` bytes to `out` and returns how many bytes the whole answer has (so a caller that
/// gave too little room sees a number above `cap`, allocates and asks again); a negative number is a refusal. `out` may be null with `cap` 0
/// to ask only for the size.
///
/// # Safety
/// `out` must be null or point to `cap` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn iyakki_core_hello(out: *mut u8, cap: usize) -> i64 {
    let text = hello_json();
    if out.is_null() {
        return if cap == 0 { text.len() as i64 } else { ERR_NULL as i64 };
    }
    let n = text.len().min(cap);
    std::ptr::copy_nonoverlapping(text.as_ptr(), out, n);
    text.len() as i64
}

/// The `hello` text (also what the tests read).
pub fn hello_json() -> String {
    let h = hello_for("", "", Capabilities::none());
    serde_json::to_string(&h).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_is_written_and_a_null_place_is_refused() {
        let (mut a, mut b, mut c) = (9u32, 9u32, 9u32);
        assert_eq!(unsafe { iyakki_core_abi_version(&mut a, &mut b, &mut c) }, OK);
        assert_eq!((a, b, c), (ABI_MAJOR, ABI_MINOR, ABI_PATCH));
        assert_eq!(unsafe { iyakki_core_abi_version(std::ptr::null_mut(), &mut b, &mut c) }, ERR_NULL);
    }

    #[test]
    fn hello_is_sized_first_then_read_whole_and_a_short_buffer_gets_the_needed_size() {
        let need = unsafe { iyakki_core_hello(std::ptr::null_mut(), 0) };
        assert!(need > 20, "{need}");
        assert!(unsafe { iyakki_core_hello(std::ptr::null_mut(), 4) } < 0, "a null place with room claimed is refused");
        let mut small = [0u8; 8];
        assert_eq!(unsafe { iyakki_core_hello(small.as_mut_ptr(), small.len()) }, need, "a short buffer is told the size it needs");
        let mut big = vec![0u8; need as usize];
        assert_eq!(unsafe { iyakki_core_hello(big.as_mut_ptr(), big.len()) }, need);
        let v: serde_json::Value = serde_json::from_slice(&big).unwrap();
        assert_eq!(v["contract"]["major"], 0);
        assert!(v["domains"].as_array().is_some_and(|d| !d.is_empty()), "the domains this build knows");
        assert!(v.get("machine").is_some_and(|m| m == ""), "no machine name leaves through the library");
    }
}
