//! Vault unlock from a new place (F-SDK-205): a counter the vault's sign-in keeps per person.
//!
//! A "place" is the network the machine is on when the vault opens: the default route's gateway address and interface, hashed with the
//! person's name so the file holds no address. The first unlock only records the place (there is nothing to compare it with); an unlock
//! from a place that is not in the person's list counts one and adds it. The list keeps the 32 newest places. Only counts leave this
//! module: `appevent_more` turns the count into `security.alert kind=vault_unlock_new_place`.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const MAX_PLACES: usize = 32;

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Book {
    places: Vec<String>,
    new_unlocks: u64,
}

fn file(dir: &Path, user: &str) -> PathBuf {
    let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    dir.join(format!("unlock-places-{who}.json"))
}

/// The hash a place is kept as.
pub fn place_key(user: &str, place: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"iyakki-unlock-place\0");
    h.update(user.as_bytes());
    h.update([0]);
    h.update(place.as_bytes());
    h.finalize().iter().take(12).map(|b| format!("{b:02x}")).collect()
}

/// The place from the kernel's routing table text: `interface@gateway` of the default route, or empty when there is none.
pub fn place_from_routes(route_table: &str) -> String {
    for l in route_table.lines().skip(1) {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() > 2 && f[1] == "00000000" {
            return format!("{}@{}", f[0], f[2]);
        }
    }
    String::new()
}

/// The place this machine is in now (empty when it has no default route: nothing is recorded or counted then).
pub fn place_now() -> String {
    place_from_routes(&iyakki_core_local::net::ipv4_routes_text())
}

/// A vault of `user` was unlocked at `place`. True when the place is new to a person who had other places already.
pub fn note_unlock(dir: &Path, user: &str, place: &str) -> bool {
    if place.is_empty() {
        return false;
    }
    let path = file(dir, user);
    let mut book: Book = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let key = place_key(user, place);
    if book.places.contains(&key) {
        return false;
    }
    let had_places = !book.places.is_empty();
    book.places.push(key);
    if book.places.len() > MAX_PLACES {
        book.places.remove(0);
    }
    if had_places {
        book.new_unlocks += 1;
    }
    if let Ok(t) = serde_json::to_string(&book) {
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, t).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
    had_places
}

/// How many unlocks from a new place the person's vault has seen (kept across restarts).
pub fn new_unlocks(dir: &Path, user: &str) -> u64 {
    std::fs::read_to_string(file(dir, user)).ok().and_then(|t| serde_json::from_str::<Book>(&t).ok()).map(|b| b.new_unlocks).unwrap_or(0)
}

#[cfg(test)]
#[path = "unlockplaces_tests.rs"]
mod tests;
