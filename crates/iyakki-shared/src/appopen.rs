//! A tool in the chat asks the shell to open one of its apps on one thing (the PDF viewer on a file and page). The tool only queues a
//! request; the shell takes it from the chat the next time it looks, applies the person's app rules and opens the app (`App::take_request`).
//! Nothing here opens anything by itself, and a request waits for nobody else than the person it was made for.

use std::sync::Mutex;

/// (person, app id, item the app is asked to show)
static QUEUE: Mutex<Vec<(String, String, String)>> = Mutex::new(Vec::new());

/// At most this many requests wait at once.
const MAX_WAITING: usize = 8;

/// Ask for `app` (its id, `pdf`) to open on `item` for `user`. An older request for the same app of the same person is replaced.
pub fn request(user: &str, app: &str, item: &str) {
    let mut q = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    q.retain(|(u, a, _)| !(u == user && a == app));
    q.push((user.into(), app.into(), item.into()));
    while q.len() > MAX_WAITING {
        q.remove(0);
    }
}

/// The oldest request waiting for `user`.
pub fn take(user: &str) -> Option<(String, String)> {
    let mut q = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    let i = q.iter().position(|(u, _, _)| u == user)?;
    let (_, app, item) = q.remove(i);
    Some((app, item))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_waits_for_its_own_person_and_a_newer_one_replaces_it() {
        request("ann-appopen", "pdf", "file:/a.pdf");
        request("bob-appopen", "pdf", "file:/b.pdf");
        request("ann-appopen", "pdf", "file:/c.pdf");
        assert_eq!(take("ann-appopen"), Some(("pdf".into(), "file:/c.pdf".into())));
        assert_eq!(take("ann-appopen"), None);
        assert_eq!(take("bob-appopen"), Some(("pdf".into(), "file:/b.pdf".into())));
        assert_eq!(take("nobody-appopen"), None);
    }
}
