//! The assistant's request to open a shell in a pod (F-K8S-26): the chat tool `k8s_exec_terminal` queues the item (`pod:` and six lines, the K8S tile
//! writes the same text) and the shell takes it on its next beat, opens the Terminal on it and keeps the screen the person is on aside. The
//! Terminal then asks the person for the whole session (a card naming the pod) and the core checks the account's terminal level, so the tool only
//! asks, it never starts anything. One request waits; a newer one replaces it.

use std::sync::Mutex;

static WAITING: Mutex<Option<String>> = Mutex::new(None);

/// Queue the item of a pod session.
pub fn queue(item: &str) {
    *WAITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(item.to_string());
}

/// The waiting item, once.
pub fn take() -> Option<String> {
    WAITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_request_waits_a_newer_one_replaces_it_and_it_is_taken_once() {
        let _lock = iyakki_testkit::lock("podterm-queue");
        let _ = take();
        assert_eq!(take(), None);
        queue("pod:a");
        queue("pod:b");
        assert_eq!(take().as_deref(), Some("pod:b"));
        assert_eq!(take(), None);
    }
}
