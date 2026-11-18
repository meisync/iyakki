//! The microphones for VOICE: the browser's in a browser session, the machine's otherwise (the browser part is `iyakki_shared::browsersource`).

use std::sync::Arc;

use iyakki_shared::browsersource::{browser_session, BrowserRecorders};
use iyakki_shared::voicedesk::Recorders;

pub fn recorders_for(user: &str) -> Arc<dyn Recorders> {
    match browser_session(user) {
        Some(m) => Arc::new(BrowserRecorders { media: m }),
        None => Arc::new(crate::voicedesk::SystemRecorders),
    }
}
