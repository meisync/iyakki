//! The kernel facts sound routing is decided from (F-ARCH-06, KI-852; decision D-ARCH-06-AUDIOROUTE in docs/development/ALPHA-7-DECISIONS.md).
//!
//! Which output IYAKKI plays on, which monitor is behind an HDMI pin, whether the Apple codec is still starting and who holds a
//! playback device are decisions of the agent's sound service. What they read is not: `/proc/asound`, the DMI product name, the
//! processes that hold `/dev/snd/pcmCcDdp`, the mixer's controls. [`AudioRoute`] is that read side: plain reads, no switching. The
//! choosing, the learned format and the software volume stay in the service and ask this trait; `iyakki-core-local` reads the
//! machine, `iyakki-core-fake` answers from a made-up tree, and the guarded handle refuses without `system.read`.

/// A program that has a playback device open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PcmHolder {
    pub pid: u32,
    /// The process name (`comm`), empty when it could not be read.
    pub name: String,
}

/// Reads of the sound kernel facts. Paths are relative to `/proc/asound` and never leave it (a name with `..` or a leading `/` reads nothing).
pub trait AudioRoute: Send + Sync {
    /// The text of `/proc/asound/<rel>` (`cards`, `pcm`, `card0/pcm0p/info`, `card0/eld#0.0`, `card0/codec#0`), `None` when it is not there.
    fn asound_text(&self, _rel: &str) -> Option<String> {
        None
    }
    /// The entry names of the directory `/proc/asound/<rel>` (`""` is `/proc/asound`), sorted; empty when it is not there.
    fn asound_dir(&self, _rel: &str) -> Vec<String> {
        Vec::new()
    }
    /// Is there a file or directory `/proc/asound/<rel>`?
    fn asound_has(&self, _rel: &str) -> bool {
        false
    }
    /// The machine's DMI product name (`MacBookPro14,1`), `None` when there is none.
    fn product_name(&self) -> Option<String> {
        None
    }
    /// The programs (other than this one) that have playback device `device` of `card` open, by pid.
    fn pcm_holders(&self, _card: u32, _device: u32) -> Vec<PcmHolder> {
        Vec::new()
    }
    /// The parent process of `pid` (`/proc/<pid>/stat`), `None` when it is gone or unreadable.
    fn process_parent(&self, _pid: u32) -> Option<u32> {
        None
    }
    /// The name (`comm`) of process `pid`, `None` when it is gone or unreadable.
    fn process_name(&self, _pid: u32) -> Option<String> {
        None
    }
    /// What the mixer says about `card` (`amixer -c N contents`), `None` when there is no mixer or it fails.
    fn mixer_contents(&self, _card: u32) -> Option<String> {
        None
    }
    /// The kernel's list of sound cards (`asound/cards`), empty when it cannot be read: the one reader of that file.
    fn cards_text(&self) -> String {
        self.asound_text("cards").unwrap_or_default()
    }
    /// Is this the route of the running machine (its real `/proc`), not a made-up tree? Only then is the selector's watched picture the
    /// right answer for the outputs, and only then may the real mixer be asked.
    fn is_machine(&self) -> bool {
        false
    }
    /// How many sound cards the kernel lists.
    fn card_count(&self) -> usize {
        self.cards_text().lines().filter(|l| l.trim_start().starts_with(|c: char| c.is_ascii_digit())).count()
    }
}

/// The handle of a machine that does not say otherwise: no sound facts.
pub struct NoAudioRoute;

impl AudioRoute for NoAudioRoute {}

/// Is `rel` a path that stays under `/proc/asound`? (Used by every implementation.)
pub fn rel_is_safe(rel: &str) -> bool {
    !rel.starts_with('/') && !rel.split('/').any(|p| p == "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_machine_with_no_route_knows_nothing_and_paths_must_stay_inside() {
        let n = NoAudioRoute;
        assert!(n.asound_text("cards").is_none() && n.asound_dir("").is_empty() && !n.asound_has("cards") && n.pcm_holders(0, 0).is_empty());
        assert_eq!(n.card_count(), 0);
        assert!(n.process_parent(1).is_none() && n.process_name(1).is_none() && !n.is_machine());
        assert!(rel_is_safe("card0/pcm0p/info") && rel_is_safe("") && !rel_is_safe("../cmdline") && !rel_is_safe("/etc/passwd") && !rel_is_safe("card0/../../x"));
    }
}
