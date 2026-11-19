//! The writer thread of the USB writer window (F-PKG-08): turns the window's `Action::Write` into the real write and sends
//! `Event::Progress` and `Event::Finished` back. The steps (free the stick, write, wipe the tail, read back) are a trait so the order
//! and the refusals are tested with a fake; `RealSteps` is the same code the terminal writer runs (`crate::writer`).
//!
//! Order, always: unmount what is mounted -> write -> wipe the old backup table at the end (only on a real block device) -> read back
//! and compare. A step that fails stops the run; later steps never run, and the message says not to boot the stick.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use crate::drives::Drive;
use crate::writer::{self, Phase, Progress, WriteReport};
use crate::writerwin::{Action, Event};

pub trait Steps {
    /// Free the stick: unmount every mounted part of it.
    fn unmount(&mut self, drive: &Drive) -> Result<(), String>;
    /// Write the image (with the install layout's first-sector patch when `install`).
    fn write(&mut self, image: &Path, drive: &Drive, install: bool, progress: &mut dyn FnMut(Progress)) -> Result<WriteReport, String>;
    /// Erase the last `len` bytes of the stick (a stale backup table).
    fn wipe_tail(&mut self, drive: &Drive, len: u64) -> Result<(), String>;
    /// Read back `bytes` and compare with `sha256`.
    fn verify(&mut self, drive: &Drive, bytes: u64, sha256: &str, progress: &mut dyn FnMut(Progress)) -> Result<(), String>;
}

/// What a run needs to know besides the steps.
pub struct Job {
    pub image: PathBuf,
    pub drive: Drive,
    pub install: bool,
    pub verify: bool,
}

/// Run one job to its end, reporting through `send`. The last event is always `Finished`.
pub fn run(job: &Job, steps: &mut dyn Steps, send: &mut dyn FnMut(Event)) {
    let r = run_inner(job, steps, send);
    send(Event::Finished(r));
}

fn run_inner(job: &Job, steps: &mut dyn Steps, send: &mut dyn FnMut(Event)) -> Result<(), String> {
    steps.unmount(&job.drive)?;
    let mut tell = |p: Progress| send(Event::Progress { phase: p.phase, done: p.done, total: p.total });
    let report = steps.write(&job.image, &job.drive, job.install, &mut tell).map_err(|e| format!("{e}\nThe stick may be half-written; do not boot it."))?;
    steps.wipe_tail(&job.drive, 1024 * 1024).map_err(|e| format!("could not wipe the end of the stick: {e}\nDo not boot this stick."))?;
    if job.verify {
        steps.verify(&job.drive, report.bytes, &report.sha256, &mut |p: Progress| tell(Progress { phase: Phase::Verifying, ..p })).map_err(|e| format!("{e}\nDo not boot this stick."))?;
    }
    Ok(())
}

/// Start the job of a `Write` action on its own thread; the window's renderer reads the receiving end of the channel it made.
/// `None` for any other action.
pub fn spawn(action: &Action, verify: bool, tx: Sender<Event>, mut steps: Box<dyn Steps + Send>) -> Option<std::thread::JoinHandle<()>> {
    let Action::Write { image, drive, install } = action else { return None };
    let job = Job { image: image.clone(), drive: drive.clone(), install: *install, verify };
    Some(std::thread::spawn(move || run(&job, steps.as_mut(), &mut |e| {
        let _ = tx.send(e);
    })))
}

/// The real steps: the terminal writer's functions. `unmount` is the caller's (it differs per system), handed in.
pub struct RealSteps<U: FnMut(&Drive) -> Result<(), String>> {
    pub unmount_fn: U,
    pub signature: [u8; 4],
}

impl<U: FnMut(&Drive) -> Result<(), String>> Steps for RealSteps<U> {
    fn unmount(&mut self, drive: &Drive) -> Result<(), String> {
        (self.unmount_fn)(drive)
    }
    fn write(&mut self, image: &Path, drive: &Drive, install: bool, progress: &mut dyn FnMut(Progress)) -> Result<WriteReport, String> {
        let (size, sig) = (drive.size_bytes, self.signature);
        let patch = move |off: u64, buf: &mut [u8]| -> Result<(), String> {
            if off == 0 {
                writer::patch_install_mbr(&mut buf[..512], size, sig).map(|_| ())
            } else {
                Ok(())
            }
        };
        let patcher: Option<writer::Patch> = install.then_some(&patch);
        writer::write_image(image, Path::new(&drive.path), Some(drive.size_bytes), patcher, progress)
    }
    fn wipe_tail(&mut self, drive: &Drive, len: u64) -> Result<(), String> {
        use std::os::unix::fs::FileTypeExt;
        // only a real block device has a stale backup table at its end; a file the tests write to is left alone
        if std::fs::metadata(&drive.path).is_ok_and(|m| m.file_type().is_block_device()) {
            writer::zero_tail(Path::new(&drive.path), drive.size_bytes, len)
        } else {
            Ok(())
        }
    }
    fn verify(&mut self, drive: &Drive, bytes: u64, sha256: &str, progress: &mut dyn FnMut(Progress)) -> Result<(), String> {
        writer::verify(Path::new(&drive.path), bytes, sha256, progress)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn drive(path: &str, size: u64) -> Drive {
        Drive { path: path.into(), name: "sdz".into(), model: "Stick".into(), size_bytes: size, removable: true, usb: true, read_only: false, mounts: vec![], swap: false }
    }

    #[derive(Default)]
    struct Fake {
        log: Vec<&'static str>,
        fail: Option<&'static str>,
    }
    impl Fake {
        fn step(&mut self, name: &'static str) -> Result<(), String> {
            self.log.push(name);
            if self.fail == Some(name) {
                Err(format!("{name} failed"))
            } else {
                Ok(())
            }
        }
    }
    impl Steps for Fake {
        fn unmount(&mut self, _: &Drive) -> Result<(), String> {
            self.step("unmount")
        }
        fn write(&mut self, _: &Path, _: &Drive, _: bool, progress: &mut dyn FnMut(Progress)) -> Result<WriteReport, String> {
            self.step("write")?;
            progress(Progress { phase: Phase::Writing, done: 5, total: 10, elapsed: Duration::ZERO });
            Ok(WriteReport { bytes: 10, sha256: "abc".into() })
        }
        fn wipe_tail(&mut self, _: &Drive, _: u64) -> Result<(), String> {
            self.step("wipe")
        }
        fn verify(&mut self, _: &Drive, bytes: u64, sha: &str, progress: &mut dyn FnMut(Progress)) -> Result<(), String> {
            assert_eq!((bytes, sha), (10, "abc"));
            progress(Progress { phase: Phase::Writing, done: 10, total: 10, elapsed: Duration::ZERO });
            self.step("verify")
        }
    }

    fn job(verify: bool) -> Job {
        Job { image: "i.iso".into(), drive: drive("/dev/sdz", 100), install: false, verify }
    }

    fn go(f: &mut Fake, verify: bool) -> Vec<Event> {
        let mut ev = vec![];
        run(&job(verify), f, &mut |e| ev.push(e));
        ev
    }

    #[test]
    fn the_steps_run_in_order_progress_is_sent_and_the_last_event_is_finished_ok() {
        let mut f = Fake::default();
        let ev = go(&mut f, true);
        assert_eq!(f.log, ["unmount", "write", "wipe", "verify"]);
        assert!(matches!(ev.first(), Some(Event::Progress { phase: Phase::Writing, done: 5, total: 10 })));
        assert!(ev.iter().any(|e| matches!(e, Event::Progress { phase: Phase::Verifying, .. })), "the read-back is shown as Verifying");
        assert!(matches!(ev.last(), Some(Event::Finished(Ok(())))));
    }

    #[test]
    fn a_failed_step_stops_the_run_and_nothing_after_it_runs() {
        for (bad, ran) in [("unmount", vec!["unmount"]), ("write", vec!["unmount", "write"]), ("wipe", vec!["unmount", "write", "wipe"]), ("verify", vec!["unmount", "write", "wipe", "verify"])] {
            let mut f = Fake { fail: Some(bad), ..Fake::default() };
            let ev = go(&mut f, true);
            assert_eq!(f.log, ran, "{bad}");
            let Some(Event::Finished(Err(why))) = ev.last() else { panic!("{bad}: {ev:?}") };
            assert!(why.contains(&format!("{bad} failed")) || why.contains("wipe"), "{why}");
            if bad != "unmount" {
                assert!(why.contains("do not boot") || why.contains("Do not boot"), "{why}");
            }
        }
    }

    #[test]
    fn the_read_back_can_be_switched_off() {
        let mut f = Fake::default();
        go(&mut f, false);
        assert_eq!(f.log, ["unmount", "write", "wipe"]);
    }

    #[test]
    fn only_a_write_action_starts_a_thread_and_the_events_arrive_over_the_channel() {
        let (tx, rx) = std::sync::mpsc::channel();
        assert!(spawn(&Action::Close, true, tx.clone(), Box::new(Fake::default())).is_none());
        let a = Action::Write { image: "i.iso".into(), drive: drive("/dev/sdz", 100), install: false };
        spawn(&a, true, tx, Box::new(Fake::default())).unwrap().join().unwrap();
        let all: Vec<Event> = rx.try_iter().collect();
        assert!(matches!(all.last(), Some(Event::Finished(Ok(())))));
    }

    #[test]
    fn the_real_steps_write_a_file_as_a_stick_verify_it_and_leave_a_file_target_unwiped() {
        let d = std::env::temp_dir().join(format!("iyakki-usb-wwr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let img = d.join("a.iso");
        let bytes: Vec<u8> = (0..300_000u32).map(|i| (i * 7 + 3) as u8).collect();
        std::fs::write(&img, &bytes).unwrap();
        let stick = d.join("stick.img");
        std::fs::write(&stick, vec![0xAAu8; 400_000]).unwrap();
        let drive = drive(stick.to_str().unwrap(), 400_000);
        let mut steps = RealSteps { unmount_fn: |_: &Drive| Ok(()), signature: [1, 2, 3, 4] };
        let mut ev = vec![];
        run(&Job { image: img, drive, install: false, verify: true }, &mut steps, &mut |e| ev.push(e));
        assert!(matches!(ev.last(), Some(Event::Finished(Ok(())))), "{:?}", ev.last());
        let now = std::fs::read(&stick).unwrap();
        assert_eq!(&now[..300_000], &bytes[..]);
        assert!(now[300_000..].iter().all(|b| *b == 0xAA), "a file is not wiped at the end, only a block device");
        let _ = std::fs::remove_dir_all(&d);
    }
}
