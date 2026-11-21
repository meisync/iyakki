//! The carrier of the sealed browser data to a cloud tenant (F-VAULT-11): temp folders and a fake tenant file; no real tenant, no real profile.

use super::*;
use std::sync::Mutex as M;

const MARK: &str = "FAKE-COOKIE-VALUE-CARRY";

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-carry-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn key(n: u8) -> ProfileKey {
    ProfileKey::from_bytes([n; 32])
}

fn sealed_here(name: &str, k: &ProfileKey) -> (PathBuf, ProfileStore) {
    let d = temp(name);
    let st = ProfileStore::new(&d, "alice");
    let p = st.profile_path();
    std::fs::create_dir_all(p.join("data")).unwrap();
    std::fs::write(p.join("data/cookies.sqlite"), format!("SQLite {MARK}")).unwrap();
    st.protect(k).unwrap();
    (d, st)
}

/// The tenant's file: one slot, and a switch for it to fail.
#[derive(Default)]
struct Tenant {
    held: M<Option<Vec<u8>>>,
    fail: M<Option<String>>,
}
impl ProfileCarrier for Tenant {
    fn put(&self, bytes: &[u8]) -> Result<(), String> {
        if let Some(e) = self.fail.lock().unwrap().clone() {
            return Err(e);
        }
        *self.held.lock().unwrap() = Some(bytes.to_vec());
        Ok(())
    }
    fn get(&self) -> Result<Option<Vec<u8>>, String> {
        if let Some(e) = self.fail.lock().unwrap().clone() {
            return Err(e);
        }
        Ok(self.held.lock().unwrap().clone())
    }
}

fn leftovers(d: &Path) -> usize {
    std::fs::read_dir(d.join("profile-carry")).map(|r| r.count()).unwrap_or(0)
}

#[test]
fn the_sealed_file_goes_to_the_tenant_unread_and_comes_back_only_with_the_same_key_leaving_nothing_behind() {
    let k = key(1);
    let (d, st) = sealed_here("a", &k);
    let t = Tenant::default();
    let n = send_with(&d, "alice", &t, 1_000).unwrap();
    let held = t.held.lock().unwrap().clone().unwrap();
    assert_eq!(n as usize, held.len());
    assert_eq!(held, std::fs::read(d.join("web-profile/alice.sealed")).unwrap(), "the tenant holds the sealed file as it is");
    assert!(!held.windows(MARK.len()).any(|w| w == MARK.as_bytes()), "the tenant holds nothing readable");
    assert_eq!(leftovers(&d), 0, "no scratch copy is left");
    assert!(st.lease(1_001).is_none(), "the claim was given back");
    // the other computer: another data folder, same person and key
    let d2 = temp("b");
    let wrong = fetch_with(&d2, "alice", &t, &key(2), 2_000).unwrap_err();
    assert!(!wrong.is_empty());
    assert_eq!(ProfileStore::new(&d2, "alice").status(), crate::profilestore::Status::Off, "a wrong key changes nothing");
    assert_eq!(leftovers(&d2), 0);
    fetch_with(&d2, "alice", &t, &k, 3_000).unwrap();
    let other = ProfileStore::new(&d2, "alice");
    assert_eq!(other.status(), crate::profilestore::Status::Sealed);
    other.open(&k).unwrap();
    assert!(std::fs::read_to_string(other.profile_path().join("data/cookies.sqlite")).unwrap().contains(MARK));
}

#[test]
fn nothing_held_a_failing_tenant_and_open_data_are_said_plainly_and_change_nothing() {
    let k = key(1);
    let (d, st) = sealed_here("c", &k);
    let t = Tenant::default();
    // the tenant holds nothing
    assert!(fetch_with(&d, "alice", &t, &k, 1).unwrap_err().contains("holds no browser data"));
    // the tenant fails: the error comes through and the scratch copy is gone
    *t.fail.lock().unwrap() = Some("the tenant signed you out: sign in again".into());
    assert_eq!(send_with(&d, "alice", &t, 1_000), Err("the tenant signed you out: sign in again".to_string()));
    assert_eq!(leftovers(&d), 0);
    assert!(st.lease(1_001).is_none());
    *t.fail.lock().unwrap() = None;
    // open data is never sent
    st.open(&k).unwrap();
    assert!(send_with(&d, "alice", &t, 2_000).is_err());
    assert!(t.held.lock().unwrap().is_none(), "nothing reached the tenant");
    // a claim held by another computer blocks the move and names it
    st.seal(&k).unwrap();
    st.take_lease("kitchen-laptop", 3_000, 300).unwrap();
    let e = send_with(&d, "alice", &t, 3_001).unwrap_err();
    assert!(e.contains("kitchen-laptop"), "{e}");
    assert!(t.held.lock().unwrap().is_none());
}

#[test]
fn the_registry_names_the_signed_in_tenants_and_refuses_one_that_is_not() {
    // a name no other test uses: the registry is process wide
    let name = "carry-registry-tenant";
    assert!(!names().contains(&name.to_string()));
    let (d, _st) = sealed_here("d", &key(1));
    assert!(send(&d, "alice", name, 1).unwrap_err().contains("is not signed in here"));
    set(name, Some(Arc::new(Tenant::default())));
    assert!(names().contains(&name.to_string()));
    assert!(send(&d, "alice", name, 5_000).is_ok());
    assert!(fetch(&d, "alice", name, "not a key", 5_001).unwrap_err().contains("not valid"));
    set(name, None);
    assert!(!names().contains(&name.to_string()));
}
