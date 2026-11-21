//! The sound kernel facts of this machine (`iyakki_core_api::AudioRoute`, F-ARCH-06): `/proc/asound`, the DMI product name, the
//! processes that hold `/dev/snd/pcmCcDdp` and `amixer -c N contents`. The agent's output selector (`audioout`) reads through this
//! and keeps only the decisions. Roots are parameters so a test builds a made-up tree.

use iyakki_core_api::audioroute::rel_is_safe;
use iyakki_core_api::{AudioRoute, PcmHolder};
use std::path::{Path, PathBuf};

pub struct LocalAudioRoute {
    proc: PathBuf,
    sys: PathBuf,
    /// Run the mixer (`None`: no mixer: a made-up tree never meets the real one).
    use_mixer: bool,
}

impl LocalAudioRoute {
    /// This machine: `/proc`, `/sys` and the real `amixer`.
    pub fn system() -> LocalAudioRoute {
        LocalAudioRoute { proc: PathBuf::from("/proc"), sys: PathBuf::from("/sys"), use_mixer: true }
    }
    /// A tree under these roots, with no mixer.
    pub fn at(proc: &Path, sys: &Path) -> LocalAudioRoute {
        LocalAudioRoute { proc: proc.to_path_buf(), sys: sys.to_path_buf(), use_mixer: false }
    }
    /// The same as [`system`](Self::system) when `proc` is `/proc`, else a tree under `proc` with the mixer off.
    pub fn for_proc(proc: &Path) -> LocalAudioRoute {
        if proc == Path::new("/proc") { Self::system() } else { Self::at(proc, &proc.join("../sys")) }
    }
    fn asound(&self, rel: &str) -> Option<PathBuf> {
        rel_is_safe(rel).then(|| self.proc.join("asound").join(rel))
    }
    fn read(p: &Path) -> Option<String> {
        std::fs::read_to_string(p).ok()
    }
}

fn is_pid(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit())
}

impl AudioRoute for LocalAudioRoute {
    fn asound_text(&self, rel: &str) -> Option<String> {
        Self::read(&self.asound(rel)?)
    }
    fn asound_dir(&self, rel: &str) -> Vec<String> {
        let Some(dir) = self.asound(rel) else { return Vec::new() };
        let mut v: Vec<String> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }
    fn asound_has(&self, rel: &str) -> bool {
        self.asound(rel).is_some_and(|p| p.exists())
    }
    fn product_name(&self) -> Option<String> {
        Self::read(&self.sys.join("class/dmi/id/product_name")).map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
    }
    fn pcm_holders(&self, card: u32, device: u32) -> Vec<PcmHolder> {
        let want = format!("/dev/snd/pcmC{card}D{device}p");
        let me = std::process::id().to_string();
        let mut out = Vec::new();
        for e in std::fs::read_dir(&self.proc).into_iter().flatten().flatten() {
            let pid = e.file_name().to_string_lossy().into_owned();
            if !is_pid(&pid) || pid == me {
                continue;
            }
            let holds = std::fs::read_dir(e.path().join("fd")).into_iter().flatten().flatten().any(|f| std::fs::read_link(f.path()).is_ok_and(|l| l.as_os_str() == want.as_str()));
            if holds {
                let name = Self::read(&e.path().join("comm")).unwrap_or_default().trim().to_string();
                if let Ok(pid) = pid.parse() {
                    out.push(PcmHolder { pid, name });
                }
            }
        }
        out.sort_by_key(|h| h.pid);
        out
    }
    fn is_machine(&self) -> bool {
        self.use_mixer
    }
    fn process_parent(&self, pid: u32) -> Option<u32> {
        // the field after the command name, which may hold spaces and brackets
        let t = Self::read(&self.proc.join(pid.to_string()).join("stat"))?;
        let rest = &t[t.rfind(')')? + 1..];
        rest.split_whitespace().nth(1)?.parse().ok()
    }
    fn process_name(&self, pid: u32) -> Option<String> {
        Self::read(&self.proc.join(pid.to_string()).join("comm")).map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
    }
    fn mixer_contents(&self, card: u32) -> Option<String> {
        if !self.use_mixer {
            return None;
        }
        let out = std::process::Command::new("amixer").args(["-c", &card.to_string(), "contents"]).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-audioroute-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_cards_the_directory_the_product_and_the_holders_are_read_from_the_given_roots_and_paths_cannot_leave() {
        let d = tree("a");
        let (proc, sys) = (d.join("proc"), d.join("sys"));
        std::fs::create_dir_all(proc.join("asound/card0/pcm0p")).unwrap();
        std::fs::write(proc.join("asound/cards"), " 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n").unwrap();
        std::fs::write(proc.join("asound/card0/pcm0p/info"), "id: ALC\n").unwrap();
        std::fs::create_dir_all(sys.join("class/dmi/id")).unwrap();
        std::fs::write(sys.join("class/dmi/id/product_name"), "MacBookPro14,1\n").unwrap();
        std::fs::write(proc.join("cmdline"), "secret").unwrap();
        // a process (not this one) with the device open and a name
        std::fs::create_dir_all(proc.join("4242/fd")).unwrap();
        std::os::unix::fs::symlink("/dev/snd/pcmC0D0p", proc.join("4242/fd/7")).unwrap();
        std::fs::write(proc.join("4242/comm"), "mpv\n").unwrap();
        std::fs::create_dir_all(proc.join("4343/fd")).unwrap();
        std::os::unix::fs::symlink("/dev/null", proc.join("4343/fd/3")).unwrap();
        let r = LocalAudioRoute::at(&proc, &sys);
        assert_eq!(r.card_count(), 1);
        assert_eq!(r.asound_dir(""), vec!["card0", "cards"]);
        assert!(r.asound_has("card0/pcm0p") && !r.asound_has("card9"));
        assert_eq!(r.asound_text("card0/pcm0p/info").unwrap(), "id: ALC\n");
        assert_eq!(r.product_name().as_deref(), Some("MacBookPro14,1"));
        assert_eq!(r.pcm_holders(0, 0), vec![PcmHolder { pid: 4242, name: "mpv".into() }]);
        assert!(r.pcm_holders(0, 1).is_empty());
        std::fs::write(proc.join("4242/stat"), "4242 (mpv (x) y) S 77 4242 4242 0\n").unwrap();
        assert_eq!(r.process_parent(4242), Some(77));
        assert_eq!(r.process_name(4242).as_deref(), Some("mpv"));
        assert!(r.process_parent(9999).is_none() && r.process_name(9999).is_none() && r.process_name(4343).is_none());
        assert!(r.asound_text("../cmdline").is_none() && r.asound_dir("../").is_empty() && !r.asound_has("../cmdline"));
        assert!(r.mixer_contents(0).is_none(), "a made-up tree never meets the real mixer");
        assert!(!r.is_machine() && LocalAudioRoute::system().is_machine() && !LocalAudioRoute::for_proc(&proc).is_machine());
        let _ = std::fs::remove_dir_all(&d);
    }
}
