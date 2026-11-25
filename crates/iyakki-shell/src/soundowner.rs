//! Whose sound the machine plays (docs/development/CLIENT-SPLIT-DESIGN.md 7b rule 3). EXPERIMENTAL, with the client media half.
//!
//! The machine has one mixer and no per-person sources, so the sound belongs to one person: the one who signed in last and has not
//! signed out. A client session of that person may take the sound to its device (`clientmedia::AudioRoutes`); a client session of
//! anybody else gets nothing and the machine's speakers are never silenced for it. With nobody signed in nothing is sent (fail closed).

use iyakki_shared::clientmedia::{AudioRoutes};
use std::sync::{Arc, Mutex, OnceLock};

/// The people signed in on this machine, oldest first; the last one owns the sound.
#[derive(Default)]
pub struct Owners {
    signed_in: Mutex<Vec<String>>,
}

impl Owners {
    fn list(&self) -> std::sync::MutexGuard<'_, Vec<String>> {
        self.signed_in.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn owner(&self) -> Option<String> {
        self.list().last().cloned()
    }

    pub fn is_owner(&self, person: &str) -> bool {
        self.owner().as_deref() == Some(person)
    }

    /// `person` signed in: the sound is theirs now. Returns the previous owner when the sound changed hands (their sound sources must end).
    pub fn signed_in(&self, person: &str) -> Option<String> {
        let mut l = self.list();
        let before = l.last().cloned().filter(|p| p != person);
        l.retain(|p| p != person);
        l.push(person.to_string());
        before
    }

    pub fn signed_out(&self, person: &str) {
        self.list().retain(|p| p != person);
    }
}

/// Put the hook on a set of routes (the process-wide ones in the product, a private set in tests).
pub fn install_on(routes: &AudioRoutes, owners: Arc<Owners>) {
    routes.set_audible(move |p| owners.is_owner(p));
}

static OWNERS: OnceLock<Arc<Owners>> = OnceLock::new();

fn global() -> &'static Arc<Owners> {
    OWNERS.get_or_init(|| {
        let o = Arc::new(Owners::default());
        install_on(&iyakki_apps::agent().client_media().audio_routes(), o.clone());
        // the end-of-session hook of the client sessions' sound: the person's routes go (D-SESSION-END)
        iyakki_shared::sessionend::global().register("client-sound", iyakki_shared::sessionend::Kind::Sound, |c| iyakki_apps::agent().client_media().audio_routes().close_person(&c.user) > 0);
        // the CAMERA tile's camera of this machine: cut at sign-out, lock and suspend even when the tile is alive (D-SESSION-END)
        iyakki_shared::sessionend::global().register("camera-device", iyakki_shared::sessionend::Kind::Capture, |_| iyakki_apps::camera::CAMERA_STOPS.stop_all() > 0);
        o
    })
}

/// A person signed in on this machine: the sound is theirs now. Returns the previous owner when the sound changed hands.
pub fn signed_in(person: &str) -> Option<String> {
    global().signed_in(person)
}

pub fn signed_out(person: &str) {
    global().signed_out(person);
}

/// Whether the machine's sound is this person's right now (asked before they are taken off the list). With nobody on the list the sound
/// is nobody's and a source with no recorded owner is treated as theirs, so a sign-out never leaves a player running.
pub fn is_owner(person: &str) -> bool {
    let g = global();
    g.is_owner(person) || g.owner().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_core_api::client::media::Side;

    fn pcm() -> Vec<i16> {
        vec![1000i16; 48_000 / 50 * 2]
    }

    #[test]
    fn a_second_person_signing_in_names_the_one_who_lost_the_sound() {
        let owners = Owners::default();
        assert_eq!(owners.signed_in("maya"), None);
        assert_eq!(owners.signed_in("maya"), None, "the same person again is no change");
        assert_eq!(owners.signed_in("sam").as_deref(), Some("maya"));
        owners.signed_out("sam");
        assert_eq!(owners.owner().as_deref(), Some("maya"));
    }

    #[test]
    fn the_sound_belongs_to_the_person_signed_in_last_and_fails_closed() {
        let (routes, owners) = (AudioRoutes::new(), Arc::new(Owners::default()));
        install_on(&routes, owners.clone());
        routes.open(1, "maya", Side::Client);
        routes.open(2, "sam", Side::Client);
        assert!(!routes.tap(48_000, 2, &pcm()), "nobody signed in: nothing is sent");
        assert!(!routes.machine_muted(), "and the machine's speakers are not silenced");
        owners.signed_in("maya");
        assert!(routes.tap(48_000, 2, &pcm()), "the owner's client route takes the sound");
        assert!(!routes.pull(1).chunks.is_empty(), "queued for that session");
        assert!(routes.pull(2).chunks.is_empty(), "a person without the sound gets none");
        owners.signed_in("sam");
        assert!(owners.is_owner("sam") && !owners.is_owner("maya"));
        owners.signed_out("sam");
        assert!(owners.is_owner("maya"), "when the last one leaves the sound goes back to the earlier person");
        owners.signed_out("maya");
        assert!(owners.owner().is_none());
    }

    #[test]
    fn a_person_without_a_client_route_has_nothing_sent_and_nothing_muted() {
        let (routes, owners) = (AudioRoutes::new(), Arc::new(Owners::default()));
        install_on(&routes, owners.clone());
        owners.signed_in("lee");
        assert!(!routes.tap(48_000, 2, &pcm()) && !routes.machine_muted() && !routes.listening());
    }
}
