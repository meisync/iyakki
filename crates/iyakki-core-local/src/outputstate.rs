//! Where sound plays now, as the core serves it (`Os::output_state`, F-ARCH-06). The selector that decides the output lives in the agent; it puts its
//! state here once ([`set_source`]) and everything else (the radio, the players, a tile) reads it through [`current`] or through `cx.os`, never by
//! naming the selector. Before a source is set, the answer is the neutral one: no device, no change, full volume.

use std::sync::{Arc, OnceLock};

use iyakki_core_api::soundctl::{NoOutputState, OutputState};

static SOURCE: OnceLock<Arc<dyn OutputState>> = OnceLock::new();

/// The program's selector gives its state (once; a second call is ignored).
pub fn set_source(s: Arc<dyn OutputState>) {
    let _ = SOURCE.set(s);
}

/// The state of this computer's sound output (the source when one was set).
pub fn current() -> Arc<dyn OutputState> {
    Arc::new(Hooked)
}

/// Reads the source on every call, so a handle taken before the source was set still follows it.
struct Hooked;

impl Hooked {
    fn source(&self) -> Arc<dyn OutputState> {
        SOURCE.get().cloned().unwrap_or_else(|| Arc::new(NoOutputState))
    }
}

impl OutputState for Hooked {
    fn device(&self) -> Option<String> {
        self.source().device()
    }
    fn generation(&self) -> u64 {
        self.source().generation()
    }
    fn gain(&self) -> u32 {
        self.source().gain()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Made(u64);
    impl OutputState for Made {
        fn device(&self) -> Option<String> {
            Some("plughw:0,3".into())
        }
        fn generation(&self) -> u64 {
            self.0
        }
        fn gain(&self) -> u32 {
            55
        }
    }

    #[test]
    fn a_handle_is_neutral_until_a_source_is_set_then_follows_it_and_the_first_source_stays() {
        // one test owns the process-wide source: before it is set nothing else in this crate's tests sets it
        let early = current();
        assert_eq!((early.device(), early.generation(), early.gain()), (None, 0, 100));
        set_source(Arc::new(Made(7)));
        assert_eq!((early.device().as_deref(), early.generation(), early.gain()), (Some("plughw:0,3"), 7, 55), "a handle taken before the source follows it");
        set_source(Arc::new(Made(9)));
        assert_eq!(current().generation(), 7, "a second source is ignored");
    }
}
